//! Shared validation for user-managed regular-expression redaction rules.

use serde_json::json;

use crate::error::{AppError, AppResult};

fn matches_empty(pattern: &str) -> bool {
    regex_syntax::Parser::new()
        .parse(pattern)
        .map(|hir| hir.properties().minimum_len() == Some(0))
        .unwrap_or(false)
}

pub fn validate_pattern(pattern: &str) -> AppResult<()> {
    regex::Regex::new(pattern).map_err(|e| {
        AppError::config(
            "redact_invalid_regex",
            json!({ "pattern": pattern, "error": e.to_string() }),
        )
    })?;
    if matches_empty(pattern) {
        return Err(AppError::config(
            "redact_zero_width_pattern",
            json!({ "pattern": pattern }),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_and_zero_width_patterns() {
        assert_eq!(
            validate_pattern("(").unwrap_err().code(),
            "redact_invalid_regex"
        );
        for pattern in ["", "^", "$", "a*", r"\b", r"x*y*"] {
            assert_eq!(
                validate_pattern(pattern).unwrap_err().code(),
                "redact_zero_width_pattern"
            );
        }
    }

    #[test]
    fn accepts_nonempty_pattern() {
        validate_pattern(r"secret-\d+").unwrap();
    }
}
