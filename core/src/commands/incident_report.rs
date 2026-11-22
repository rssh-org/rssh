use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

mod storage;
use storage::{Record, Store};

#[cfg(test)]
mod tests;

const EXCERPT_SEPARATOR: &str = "\n\n";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSummary {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub is_current: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncidentReport {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Excerpt {
    pub text: String,
    pub session_id: String,
    pub session_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureTarget {
    Current,
    New,
    Existing(String),
}

fn not_found() -> AppError {
    AppError::not_found("incident_report_not_found", serde_json::json!({}))
}

fn title(content: &str) -> AppResult<&str> {
    let title = content.lines().next().unwrap_or_default().trim();
    if title.is_empty() {
        return Err(AppError::config(
            "incident_report_empty_title",
            serde_json::json!({}),
        ));
    }
    Ok(title)
}

fn single_line_title(title: &str) -> String {
    title.trim().replace("\r\n", " ").replace(['\r', '\n'], " ")
}

fn fence_width(text: &str) -> usize {
    let longest = text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
    3.max(longest + 1)
}

fn is_fence(text: &str) -> bool {
    text.len() >= 3 && text.bytes().all(|ch| ch == b'`')
}

// Read only the trailing block RSSH generates. Other user-authored Markdown
// (including a judgment below the block) remains opaque.
struct CaptureBlock<'a> {
    header: &'a str, // The complete document through the session heading.
    heading: &'a str,
    heading_offset: usize,
    fence: &'a str,
    text: &'a str,
    after_fence: &'a str,
}

impl<'a> CaptureBlock<'a> {
    fn trailing(document: &'a str) -> Option<Self> {
        let block = document.trim_end_matches([' ', '\t', '\r', '\n']);
        let after_fence = &document[block.len()..];
        let (before_close, fence) = block.rsplit_once('\n')?;
        if !is_fence(fence) {
            return None;
        }
        let (header, text) = before_close.rsplit_once(&format!("\n\n{fence}\n"))?;
        let (above, heading) = header.rsplit_once("\n\n")?;
        // A standalone fence closes the block. Text below it belongs to the
        // user even when the document ends in another fence-like line.
        let closed_early = text.lines().any(|line| {
            let indent = line.len() - line.trim_start_matches(' ').len();
            let token = line.trim_start_matches(' ').trim_end_matches([' ', '\t']);
            indent <= 3 && token.len() >= fence.len() && is_fence(token)
        });
        if !heading.starts_with("## ") || heading.contains('\n') || closed_early {
            return None;
        }
        Some(Self {
            header,
            heading,
            heading_offset: above.len() + 2,
            fence,
            text,
            after_fence,
        })
    }

    fn append(&self, text: &str) -> String {
        let fence = "`".repeat(
            self.fence
                .len()
                .max(fence_width(self.text))
                .max(fence_width(text)),
        );
        format!(
            "{}\n\n{fence}\n{}{EXCERPT_SEPARATOR}{text}\n{fence}{}",
            self.header, self.text, self.after_fence
        )
    }

    fn retained_in(&self, edited: &Self) -> bool {
        // Titles may be renamed while the last log is being edited. The rest
        // of the header still identifies the same position in the document.
        let same_position = self
            .header
            .lines()
            .skip(1)
            .eq(edited.header.lines().skip(1));
        if self.heading != edited.heading {
            return false;
        }
        // When a session name recurs, deleting its final group can expose an
        // earlier session with identical text. Renaming the title at the same
        // time must not transfer the deleted group's identity to that one.
        let occurrences = |block: &Self| {
            block
                .header
                .lines()
                .skip(1)
                .filter(|line| *line == block.heading)
                .count()
        };
        same_position || (self.text == edited.text && occurrences(self) == occurrences(edited))
    }
}

fn append_new_group(content: &mut String, excerpt: &Excerpt) -> usize {
    content.push_str("\n\n");
    let offset = content.len();
    content.push_str("## ");
    for ch in excerpt.session_name.chars() {
        if matches!(
            ch,
            '\\' | '[' | ']' | '<' | '>' | '*' | '_' | '#' | '`' | '~' | '&'
        ) {
            content.push('\\');
        }
        content.push(if matches!(ch, '\r' | '\n') { ' ' } else { ch });
    }
    let fence = "`".repeat(fence_width(&excerpt.text));
    content.push_str("\n\n");
    content.push_str(&fence);
    content.push('\n');
    content.push_str(&excerpt.text);
    content.push('\n');
    content.push_str(&fence);
    offset
}

fn retained_heading(before: &str, after: &str, offset: Option<usize>) -> Option<usize> {
    let before = CaptureBlock::trailing(before)?;
    let after = CaptureBlock::trailing(after)?;
    (before.heading_offset == offset? && before.retained_in(&after)).then_some(after.heading_offset)
}

// Reapply only terminal captures that arrived after the editor took its
// snapshot. A same-session capture can change both fences as well as the body.
fn merge_captures(base: &str, draft: &str, stored: &str) -> Option<String> {
    if let Some(groups) = stored
        .strip_prefix(base)
        .filter(|tail| tail.starts_with("\n\n## "))
    {
        return Some(format!("{draft}{groups}"));
    }
    let original = CaptureBlock::trailing(base)?;
    let (new_fence, body) = stored
        .strip_prefix(original.header)?
        .strip_prefix("\n\n")?
        .split_once('\n')?;
    if !is_fence(new_fence) || new_fence.len() < original.fence.len() {
        return None;
    }
    let continuation = body
        .strip_prefix(original.text)?
        .strip_prefix(EXCERPT_SEPARATOR)?;
    let (added_text, later_groups) = continuation.split_once(&format!("\n{new_fence}"))?;
    let later_groups = later_groups.strip_prefix(original.after_fence)?;
    if !later_groups.is_empty() && !later_groups.starts_with("\n\n## ") {
        return None;
    }
    let (_, original_body) = base.split_once('\n')?;
    let (draft_title, draft_body) = draft.split_once('\n')?;
    if let Some(notes) = draft_body.strip_prefix(original_body) {
        // Title edits and notes below the original block do not overlap the
        // terminal's changes to the block itself, even when its fence widened.
        let (_, stored_body) = stored.split_once('\n')?;
        return Some(format!("{draft_title}\n{stored_body}{notes}"));
    }
    let edited = CaptureBlock::trailing(draft)?;
    if !original.retained_in(&edited) {
        return None;
    }
    let mut merged = edited.append(added_text);
    merged.push_str(later_groups);
    Some(merged)
}

pub fn list(data_dir: &Path) -> AppResult<Vec<ReportSummary>> {
    Ok(Store::open(data_dir)?.summaries())
}

pub fn load(data_dir: &Path, id: &str) -> AppResult<IncidentReport> {
    let store = Store::open(data_dir)?;
    let pos = store.record(id)?;
    let row = &store.index.reports[pos];
    let content = store.read(pos)?;
    Ok(IncidentReport {
        id: row.id.to_string(),
        title: row.title.clone(),
        created_at: row.created_at,
        content,
    })
}

pub fn select(data_dir: &Path, id: &str) -> AppResult<Vec<ReportSummary>> {
    let mut store = Store::open_for_write(data_dir)?;
    let pos = store.record(id)?;
    let current = Some(store.index.reports[pos].id);
    if store.index.current != current {
        store.index.current = current;
        store.commit()?;
    }
    Ok(store.summaries())
}

pub fn delete(data_dir: &Path, id: &str) -> AppResult<Vec<ReportSummary>> {
    let mut store = Store::open_for_write(data_dir)?;
    if let Some(pos) = store.position(id) {
        let deleted = store.index.reports.remove(pos);
        if store.index.current == Some(deleted.id) {
            store.index.current = store.index.reports.first().map(|r| r.id);
        }
        store.commit()?;
    }
    Ok(store.summaries())
}

pub fn capture(
    data_dir: &Path,
    excerpt: &Excerpt,
    target: CaptureTarget,
    default_title: &str,
) -> AppResult<Vec<ReportSummary>> {
    if excerpt.text.is_empty() {
        return Err(AppError::config(
            "incident_report_empty_excerpt",
            serde_json::json!({}),
        ));
    }
    let name = single_line_title(default_title);
    title(&name)?;
    let mut store = Store::open_for_write(data_dir)?;
    let pos = match target {
        CaptureTarget::Current => store
            .index
            .current
            .map(|id| store.record(&id.to_string()))
            .transpose()?,
        CaptureTarget::Existing(id) => Some(store.record(&id)?),
        CaptureTarget::New => None,
    };
    let (pos, mut content) = match pos {
        Some(pos) => (pos, store.read(pos)?),
        None => {
            let row = Record::new(&name);
            // Preserve insertion order for identical creation timestamps, newest first.
            let pos = store
                .index
                .reports
                .partition_point(|other| other.created_at > row.created_at);
            store.index.reports.insert(pos, row);
            (pos, name)
        }
    };

    let row = &mut store.index.reports[pos];
    let block = CaptureBlock::trailing(&content).filter(|block| {
        Some(block.heading_offset) == row.last_heading_offset
            && row.last_session_id.as_ref() == Some(&excerpt.session_id)
    });
    let offset = if let Some(block) = block {
        let offset = block.heading_offset;
        content = block.append(&excerpt.text);
        offset
    } else {
        append_new_group(&mut content, excerpt)
    };
    row.last_heading_offset = Some(offset);
    row.last_session_id = Some(excerpt.session_id.clone());
    store.index.current = Some(row.id);
    store.write(pos, &content)?;
    Ok(store.summaries())
}

pub fn update(
    data_dir: &Path,
    id: &str,
    content: &str,
    previous_content: &str,
) -> AppResult<Vec<ReportSummary>> {
    title(content)?;
    let mut store = Store::open_for_write(data_dir)?;
    let pos = store.record(id)?;
    let stored = store.read(pos)?;
    if stored == content || content == previous_content {
        return Ok(store.summaries());
    }
    let row = &mut store.index.reports[pos];
    let merged = if stored == previous_content {
        content.to_owned()
    } else {
        // Only a capture may extend a stale draft automatically.
        let conflict = || AppError::other("incident_report_edit_conflict", serde_json::json!({}));
        if previous_content.len() < row.last_edit_bytes {
            return Err(conflict());
        }
        merge_captures(previous_content, content, &stored).ok_or_else(conflict)?
    };
    row.title = title(&merged)?.to_owned();
    row.last_heading_offset = retained_heading(&stored, &merged, row.last_heading_offset);
    if row.last_heading_offset.is_none() {
        row.last_session_id = None;
    }
    row.last_edit_bytes = merged.len();
    store.write(pos, &merged)?;
    Ok(store.summaries())
}
