//! Skill 管理：编译时内嵌的只读 builtin + DB 中的用户自定义 Skill。
//! `general` 直接展开进 system prompt；其它 Skill 通过目录 + `load_skill` 按需加载。

use serde::{Deserialize, Serialize};

use crate::db::{ai_skill, Db};
use crate::error::AppResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub builtin: bool,
}

pub const GENERAL_ID: &str = "general";
pub const WEB_RESEARCH_ID: &str = "web-research";

const GENERAL_NAME: &str = "General Ops diagnosis";
const GENERAL_DESC: &str =
    "Default rule set + workflow reference for CPU / memory / general triage. The LLM picks commands itself.";
const WEB_RESEARCH_NAME: &str = "Web research";
const WEB_RESEARCH_DESC: &str =
    "Fetch and analyze concrete web URLs already present in a user message, with citation and prompt-injection safeguards. This does not search the web.";

pub fn builtin(id: &str) -> Option<SkillRecord> {
    let (name, description, content) = match id {
        GENERAL_ID => (GENERAL_NAME, GENERAL_DESC, super::prompts::GENERAL),
        WEB_RESEARCH_ID => (
            WEB_RESEARCH_NAME,
            WEB_RESEARCH_DESC,
            super::prompts::WEB_RESEARCH,
        ),
        _ => return None,
    };
    Some(SkillRecord {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        content: content.into(),
        builtin: true,
    })
}

fn user_record(user: ai_skill::UserSkill) -> SkillRecord {
    SkillRecord {
        id: user.id,
        name: user.name,
        description: user.description,
        content: user.content,
        builtin: false,
    }
}

pub fn list_all(db: &Db) -> AppResult<Vec<SkillRecord>> {
    let users = list_user(db)?;
    let mut out = vec![builtin(GENERAL_ID).expect("general builtin")];
    if !users.iter().any(|skill| skill.id == WEB_RESEARCH_ID) {
        out.push(builtin(WEB_RESEARCH_ID).expect("web-research builtin"));
    }
    out.extend(users);
    Ok(out)
}

/// 仅返回用户自定义 Skill。给会话启动时 snapshot cache 用。
pub fn list_user(db: &Db) -> AppResult<Vec<SkillRecord>> {
    Ok(ai_skill::list(db)?.into_iter().map(user_record).collect())
}

pub fn get(db: &Db, id: &str) -> AppResult<Option<SkillRecord>> {
    // Before this builtin existed, user Skill ids were free-form. Preserve a
    // preexisting `web-research` record as an override until the user deletes
    // it; otherwise an upgrade would hide data and make it impossible to edit.
    if id == WEB_RESEARCH_ID {
        if let Some(record) = ai_skill::get(db, id)? {
            return Ok(Some(user_record(record)));
        }
    }
    if let Some(record) = builtin(id) {
        return Ok(Some(record));
    }
    Ok(ai_skill::get(db, id)?.map(user_record))
}

pub fn is_builtin(id: &str) -> bool {
    matches!(id, GENERAL_ID | WEB_RESEARCH_ID)
}

pub fn save_user(db: &Db, rec: &SkillRecord) -> AppResult<()> {
    let is_legacy_override = rec.id == WEB_RESEARCH_ID && ai_skill::get(db, &rec.id)?.is_some();
    if is_builtin(&rec.id) && !is_legacy_override {
        return Err(crate::error::AppError::config(
            "skill_builtin_readonly",
            serde_json::json!({ "id": rec.id }),
        ));
    }
    ai_skill::upsert(
        db,
        &ai_skill::UserSkill {
            id: rec.id.clone(),
            name: rec.name.clone(),
            description: rec.description.clone(),
            content: rec.content.clone(),
        },
    )
}

pub fn delete_user(db: &Db, id: &str) -> AppResult<()> {
    let is_legacy_override = id == WEB_RESEARCH_ID && ai_skill::get(db, id)?.is_some();
    if is_builtin(id) && !is_legacy_override {
        return Err(crate::error::AppError::config(
            "skill_builtin_undeletable",
            serde_json::json!({ "id": id }),
        ));
    }
    ai_skill::delete(db, id)
}

/// 构造会话启动用的 system prompt：
/// - builtin general 规则集 **直接展开**（永远在 prompt 里）
/// - 其它 builtin + user Skill **只放 id + description**，用 `load_skill(<id>)` 按需加载
///
/// `user_locale_label` 是给 LLM 的回复语言提示（如 "English"、"Chinese (Simplified)"），
/// 由 commands 层根据前端 UI locale 解析后传入。
///
/// `is_mobile` = true 时追加一段移动端能力声明：`analyze_locally` 真·阻断（Tauri 2
/// mobile 不能 spawn 分析窗口），`download_file` 则是劝退（技术上能跑，但没了
/// analyze_locally，下到私有目录的文件也用不上），让 LLM 引导用户改用桌面端。
pub fn build_catalog_prompt(
    db: &Db,
    user_locale_label: &str,
    is_mobile: bool,
) -> AppResult<String> {
    let mut s = String::new();
    s.push_str(super::prompts::GENERAL);

    s.push_str("\n\n---\n\n# Available skills (catalog)\n\n");
    s.push_str(
        "The `general` skill is already active. The skills below are lazy-loaded: \
         when one matches the current problem, call `load_skill(<id>)` before following it.\n\n",
    );
    let mut loadable: Vec<SkillRecord> = ai_skill::list(db)?.into_iter().map(user_record).collect();
    if !loadable.iter().any(|skill| skill.id == WEB_RESEARCH_ID) {
        loadable.insert(0, builtin(WEB_RESEARCH_ID).expect("web-research builtin"));
    }
    for skill in loadable {
        let desc = if skill.description.is_empty() {
            "(no description)"
        } else {
            &skill.description
        };
        s.push_str(&format!(
            "- **{}** (id: `{}`) — {}\n",
            skill.name, skill.id, desc
        ));
    }

    s.push_str(&format!(
        "\n---\n\n# Response language\n\nRespond to the user in {user_locale_label}. Keep tool-call arguments (cmd, explain, side_effect, etc.) consistent with the user's language too — those are also user-facing.\n"
    ));

    if is_mobile {
        s.push_str(
            "\n---\n\n# Runtime: mobile device\n\n\
             The user is running rssh on a **mobile build** (Android / iOS). On this build the following tools are **unavailable** and MUST NOT be invoked:\n\
             - `analyze_locally` — the mobile app cannot spawn additional windows.\n\
             - `download_file` — the mobile app has no native file-save dialog.\n\n\
             If the diagnosis would normally require either tool (e.g. dump heap to local for MAT, save flamegraph SVG, run pprof locally), **do not attempt them on the remote host as a workaround** — heap dumps and similar long-running probes can cause STW pauses or fill remote disk. \
             Instead, tell the user plainly that this step needs the desktop build of rssh, and continue the diagnosis with whatever in-session tooling remains useful.\n",
        );
    }

    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_both_builtin_skills() {
        let db = Db::open_in_memory().unwrap();

        let records = list_all(&db).unwrap();

        let builtin_ids: Vec<&str> = records
            .iter()
            .filter(|skill| skill.builtin)
            .map(|skill| skill.id.as_str())
            .collect();
        assert_eq!(builtin_ids, ["general", "web-research"]);
        assert_eq!(
            get(&db, "web-research").unwrap().unwrap().content,
            super::super::prompts::WEB_RESEARCH
        );
    }

    #[test]
    fn web_research_is_read_only() {
        let db = Db::open_in_memory().unwrap();
        let record = SkillRecord {
            id: "web-research".into(),
            name: "replacement".into(),
            description: String::new(),
            content: "replacement".into(),
            builtin: false,
        };

        assert!(save_user(&db, &record).is_err());
        assert!(delete_user(&db, "web-research").is_err());
    }

    #[test]
    fn catalog_lists_web_research_without_inlining_it() {
        let db = Db::open_in_memory().unwrap();

        let prompt = build_catalog_prompt(&db, "English", false).unwrap();

        assert!(prompt.contains("`web-research`"));
        assert!(!prompt.contains(super::super::prompts::WEB_RESEARCH));
    }

    #[test]
    fn web_research_searches_before_fetching_and_verifying_sources() {
        let content = super::super::prompts::WEB_RESEARCH;
        let search = content.find("`web_search`").expect("search step");
        let fetch = content.find("`web_fetch`").expect("fetch step");

        assert!(search < fetch);
        assert!(content.contains("snippets"));
        assert!(content.contains("CAPTCHA"));
        assert!(content.contains("sensitive"));
    }

    #[test]
    fn preserves_a_preexisting_user_skill_with_the_new_builtin_id() {
        let db = Db::open_in_memory().unwrap();
        ai_skill::upsert(
            &db,
            &ai_skill::UserSkill {
                id: "web-research".into(),
                name: "My existing research workflow".into(),
                description: "legacy".into(),
                content: "keep this content".into(),
            },
        )
        .unwrap();

        let records = list_all(&db).unwrap();
        let matches: Vec<&SkillRecord> = records
            .iter()
            .filter(|skill| skill.id == "web-research")
            .collect();
        assert_eq!(matches.len(), 1);
        assert!(!matches[0].builtin);
        assert_eq!(
            get(&db, "web-research").unwrap().unwrap().content,
            "keep this content"
        );

        save_user(
            &db,
            &SkillRecord {
                id: "web-research".into(),
                name: "updated".into(),
                description: String::new(),
                content: "updated content".into(),
                builtin: false,
            },
        )
        .unwrap();
        delete_user(&db, "web-research").unwrap();

        assert!(get(&db, "web-research").unwrap().unwrap().builtin);
    }
}
