//! User skills, loaded on demand from the system prompt's catalog.

use serde::{Deserialize, Serialize};

use crate::db::{ai_skill, Db};
use crate::error::AppResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
    /// Retained for older configuration payloads; current skills are all user-owned.
    pub builtin: bool,
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
    Ok(ai_skill::list(db)?.into_iter().map(user_record).collect())
}

/// Skills available for lazy loading and the session's content snapshot.
pub fn list_user(db: &Db) -> AppResult<Vec<SkillRecord>> {
    list_all(db)
}

pub fn get(db: &Db, id: &str) -> AppResult<Option<SkillRecord>> {
    Ok(ai_skill::get(db, id)?.map(user_record))
}

pub fn save_user(db: &Db, rec: &SkillRecord) -> AppResult<()> {
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
    ai_skill::delete(db, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_database_has_no_skills() {
        let db = Db::open_in_memory().unwrap();
        assert!(list_all(&db).unwrap().is_empty());
    }

    #[test]
    fn user_skills_can_be_edited_and_deleted_without_changing_system_prompt() {
        let db = Db::open_in_memory().unwrap();
        let system_prompt = super::super::prompts::get(&db).unwrap();
        let mut record = SkillRecord {
            id: "user-mine".into(),
            name: "My workflow".into(),
            description: "Custom workflow".into(),
            content: "Original".into(),
            builtin: false,
        };
        save_user(&db, &record).unwrap();
        record.content = "Updated".into();
        save_user(&db, &record).unwrap();
        let saved = get(&db, &record.id).unwrap().unwrap();
        assert_eq!(saved.content, "Updated");
        assert!(!saved.builtin);
        assert_eq!(list_user(&db).unwrap().len(), 1);
        delete_user(&db, &record.id).unwrap();
        assert!(get(&db, &record.id).unwrap().is_none());
        assert_eq!(super::super::prompts::get(&db).unwrap(), system_prompt);
    }
}
