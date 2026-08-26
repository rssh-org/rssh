//! Skill 管理：编译时内嵌的只读 builtin `general` + DB 中的用户自定义 Skill。
//! `general` 直接展开进 system prompt；用户自定义 Skill 通过目录 + `load_skill` 按需加载。

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

const GENERAL_NAME: &str = "General Ops diagnosis";
const GENERAL_DESC: &str = "Default rule set + workflow reference for CPU / memory / general triage. The LLM picks commands itself.";

pub fn builtin(id: &str) -> Option<SkillRecord> {
    let (name, description, content) = match id {
        GENERAL_ID => (GENERAL_NAME, GENERAL_DESC, super::prompts::GENERAL),
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
    let mut out = vec![builtin(GENERAL_ID).expect("general builtin")];
    out.extend(list_user(db)?);
    Ok(out)
}

/// 仅返回用户自定义 Skill。给会话启动时 snapshot cache 用。
pub fn list_user(db: &Db) -> AppResult<Vec<SkillRecord>> {
    Ok(ai_skill::list(db)?.into_iter().map(user_record).collect())
}

pub fn get(db: &Db, id: &str) -> AppResult<Option<SkillRecord>> {
    if let Some(record) = builtin(id) {
        return Ok(Some(record));
    }
    Ok(ai_skill::get(db, id)?.map(user_record))
}

pub fn is_builtin(id: &str) -> bool {
    matches!(id, GENERAL_ID)
}

pub fn save_user(db: &Db, rec: &SkillRecord) -> AppResult<()> {
    if is_builtin(&rec.id) {
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
    if is_builtin(id) {
        return Err(crate::error::AppError::config(
            "skill_builtin_undeletable",
            serde_json::json!({ "id": id }),
        ));
    }
    ai_skill::delete(db, id)
}

/// 构造会话启动用的 system prompt：
/// - builtin general 规则集 **直接展开**（永远在 prompt 里）
/// - 用户自定义 Skill **只放 id + description**，用 `load_skill(<id>)` 按需加载
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

    let loadable = list_user(db)?;
    if !loadable.is_empty() {
        s.push_str("\n\n---\n\n# Available skills (catalog)\n\n");
        s.push_str(
            "The `general` skill is already active. The skills below are lazy-loaded: \
             when one matches the current problem, call `load_skill(<id>)` before following it.\n\n",
        );
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
    fn exposes_only_general_builtin() {
        let db = Db::open_in_memory().unwrap();

        let records = list_all(&db).unwrap();

        let builtin_ids: Vec<&str> = records
            .iter()
            .filter(|skill| skill.builtin)
            .map(|skill| skill.id.as_str())
            .collect();
        assert_eq!(builtin_ids, ["general"]);
    }

    #[test]
    fn general_builtin_is_read_only() {
        let db = Db::open_in_memory().unwrap();
        let record = SkillRecord {
            id: "general".into(),
            name: "replacement".into(),
            description: String::new(),
            content: "replacement".into(),
            builtin: false,
        };

        assert!(save_user(&db, &record).is_err());
        assert!(delete_user(&db, "general").is_err());
    }

    #[test]
    fn catalog_omitted_when_no_user_skills() {
        let db = Db::open_in_memory().unwrap();

        let prompt = build_catalog_prompt(&db, "English", false).unwrap();

        // The catalog section appears only when user skills exist. The
        // `load_skill` tool itself is always listed in general.md regardless.
        assert!(!prompt.contains("Available skills (catalog)"));
    }

    #[test]
    fn catalog_lists_user_skills_only() {
        let db = Db::open_in_memory().unwrap();
        ai_skill::upsert(
            &db,
            &ai_skill::UserSkill {
                id: "user-mine".into(),
                name: "My workflow".into(),
                description: "custom".into(),
                content: "keep this content".into(),
            },
        )
        .unwrap();

        let prompt = build_catalog_prompt(&db, "English", false).unwrap();

        assert!(prompt.contains("`user-mine`"));
        assert!(prompt.contains("My workflow"));
        assert!(prompt.contains("load_skill"));
    }
}
