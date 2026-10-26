/// A remote file name is a suggested name, never an authorized local path.
pub fn valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_remote_names_that_can_escape_the_picker_target() {
        for name in [
            "",
            ".",
            "..",
            "../master.key",
            "/data/private/key",
            "a/b",
            "a\\b",
            "a\0b",
            "a\nb",
        ] {
            assert!(!valid_file_name(name), "accepted {name:?}");
        }
    }

    #[test]
    fn preserves_ordinary_unicode_and_hidden_file_names() {
        for name in ["config.json", ".env", "备份 2026.json", "report..txt"] {
            assert!(valid_file_name(name), "rejected {name:?}");
        }
    }
}
