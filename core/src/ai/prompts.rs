//! Editable system prompt with fixed placeholders for session context.

use std::sync::LazyLock;

use regex::{Captures, Regex};

use super::{shell::ShellKind, skills::SkillRecord};
use crate::db::{settings, Db};
use crate::error::AppResult;

pub const SYSTEM: &str = include_str!("prompts/system.md");
pub const SYSTEM_PROMPT_KEY: &str = "ai_system_prompt";

pub fn get(db: &Db) -> AppResult<String> {
    Ok(settings::get(db, SYSTEM_PROMPT_KEY)?.unwrap_or_else(|| SYSTEM.to_owned()))
}

/// Expand only placeholders in the template, never in inserted context.
/// Missing placeholders intentionally omit that context; nothing is appended.
pub fn render_system_prompt(
    template: &str,
    response_language: &str,
    skills: &[SkillRecord],
    shell: ShellKind,
) -> String {
    static PLACEHOLDERS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\{\{(TARGET_SHELL|RESPONSE_LANGUAGE|SKILLS)\}\}")
            .expect("fixed system prompt placeholders")
    });

    let shell_block = shell.system_prompt_block();
    let mut catalog = String::new();
    for skill in skills {
        let description = if skill.description.is_empty() {
            "(no description)"
        } else {
            &skill.description
        };
        catalog.push_str(&format!(
            "- **{}** (id: `{}`) — {}\n",
            skill.name, skill.id, description
        ));
    }
    if catalog.is_empty() {
        catalog.push_str("No skills are available for this session.");
    }

    PLACEHOLDERS
        .replace_all(template, |captures: &Captures<'_>| match &captures[1] {
            "TARGET_SHELL" => shell_block.as_str(),
            "RESPONSE_LANGUAGE" => response_language,
            "SKILLS" => catalog.trim_end(),
            _ => unreachable!("only fixed placeholders are matched"),
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edited_system_prompt_survives_reopening_and_replaces_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let edited = "My assistant identity.\n\nMy diagnosis method.\n";
        settings::set(&db, SYSTEM_PROMPT_KEY, edited).unwrap();
        drop(db);

        let db = Db::open(dir.path()).unwrap();
        assert_eq!(get(&db).unwrap(), edited);
        let prompt = render_system_prompt(&get(&db).unwrap(), "English", &[], ShellKind::Posix);
        assert_eq!(prompt, edited);
        assert!(super::super::skills::list_all(&db).unwrap().is_empty());
    }

    #[test]
    fn empty_catalog_has_no_loadable_entries() {
        let prompt = render_system_prompt("{{SKILLS}}", "English", &[], ShellKind::Posix);
        assert_eq!(prompt, "No skills are available for this session.");
    }

    #[test]
    fn catalog_lists_user_skills_without_loading_their_contents() {
        let db = Db::open_in_memory().unwrap();
        crate::db::ai_skill::upsert(
            &db,
            &crate::db::ai_skill::UserSkill {
                id: "user-mine".into(),
                name: "My workflow".into(),
                description: "custom".into(),
                content: "keep this content".into(),
            },
        )
        .unwrap();

        let skills = super::super::skills::list_user(&db).unwrap();
        let prompt = render_system_prompt("{{SKILLS}}", "English", &skills, ShellKind::Posix);
        assert!(prompt.contains("`user-mine`"));
        assert!(prompt.contains("My workflow"));
        assert!(!prompt.contains("keep this content"));
        assert!(!prompt.contains("load_skill"));
        assert!(!prompt.contains("general"));
    }

    #[test]
    fn default_system_prompt_expands_all_context_at_the_template_positions() {
        let db = Db::open_in_memory().unwrap();
        let template = get(&db).unwrap();
        assert_eq!(template, SYSTEM);
        let prompt = render_system_prompt(&template, "Chinese (Simplified)", &[], ShellKind::Cmd);
        assert!(prompt.contains("# Hard rules"));
        let shell_section = prompt
            .split_once("# Target shell\n\n")
            .unwrap()
            .1
            .split_once("\n# Universal methodology")
            .unwrap()
            .0;
        assert!(shell_section.contains("**cmd.exe (Windows)**"));
        assert!(shell_section.contains("%errorlevel%"));
        assert!(prompt.contains("Respond to the user in Chinese (Simplified)"));
        assert!(prompt.contains("# Available skills (catalog)\n\nNo skills are available"));
        assert_eq!(prompt.matches("# Target shell").count(), 1);
        assert_eq!(prompt.matches("# Available skills (catalog)").count(), 1);
        assert!(!prompt.contains("# Target environment"));
        for placeholder in ["{{TARGET_SHELL}}", "{{RESPONSE_LANGUAGE}}", "{{SKILLS}}"] {
            assert!(!prompt.contains(placeholder));
        }
    }

    #[test]
    fn placeholder_values_are_literal_and_not_expanded_recursively() {
        let skills = vec![SkillRecord {
            id: "user-literal".into(),
            name: "{{TARGET_SHELL}}".into(),
            description: "{{RESPONSE_LANGUAGE}} $1".into(),
            content: "private skill body".into(),
            builtin: false,
        }];
        let prompt = render_system_prompt(
            "{{SKILLS}}\n{{RESPONSE_LANGUAGE}}\n{{RESPONSE_LANGUAGE}}\n{{UNKNOWN}}",
            "English",
            &skills,
            ShellKind::Posix,
        );
        assert_eq!(prompt, "- **{{TARGET_SHELL}}** (id: `user-literal`) — {{RESPONSE_LANGUAGE}} $1\nEnglish\nEnglish\n{{UNKNOWN}}");
        assert_eq!(
            render_system_prompt("", "English", &skills, ShellKind::Posix),
            ""
        );
    }
}
