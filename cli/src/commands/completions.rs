//! Runtime shell completion and database-backed name candidates.

use std::ffi::OsStr;
use std::io;
use std::path::Path;

use clap_complete::env::{Bash, EnvCompleter, Fish, Powershell, Zsh};
use clap_complete::CompletionCandidate;
use rusqlite::{Connection, OpenFlags};

const COMPLETE_VAR: &str = "_RSSH_COMPLETE";

#[derive(Clone, Copy)]
enum NameKind {
    Profile,
    Credential,
    Forward,
    Group,
}

pub fn print_completions(shell: &str) {
    if let Err(error) = write_completions(shell, &mut io::stdout()) {
        eprintln!("Failed to generate completions: {error}");
    }
}

fn write_completions(shell: &str, output: &mut dyn io::Write) -> io::Result<()> {
    let shell: &dyn EnvCompleter = match shell {
        "bash" => &Bash,
        "zsh" => &Zsh,
        "fish" => &Fish,
        "powershell" | "pwsh" => &Powershell,
        _ => {
            eprintln!("Supported shells: zsh, bash, powershell, fish");
            return Ok(());
        }
    };

    shell.write_registration(COMPLETE_VAR, "rssh", "rssh", "rssh", output)?;
    if shell.is("zsh") {
        // clap's dynamic registration supports sourcing. When zsh autoloads
        // this file as _rssh, also dispatch the completion that loaded it.
        writeln!(
            output,
            "\nif [[ $funcstack[1] == _rssh ]]; then\n    ${{_comps[rssh]}} \"$@\"\nfi"
        )?;
    }
    Ok(())
}

pub fn complete_profiles(current: &OsStr) -> Vec<CompletionCandidate> {
    complete_names(NameKind::Profile, current)
}

pub fn complete_credentials(current: &OsStr) -> Vec<CompletionCandidate> {
    complete_names(NameKind::Credential, current)
}

pub fn complete_forwards(current: &OsStr) -> Vec<CompletionCandidate> {
    complete_names(NameKind::Forward, current)
}

pub fn complete_groups(current: &OsStr) -> Vec<CompletionCandidate> {
    complete_names(NameKind::Group, current)
}

fn complete_names(kind: NameKind, current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(current) = current.to_str() else {
        return Vec::new();
    };
    let Ok(data_dir) = rssh_core::db::data_dir() else {
        return Vec::new();
    };
    complete_names_from_db(kind, current, &data_dir.join("rssh.db"))
}

fn complete_names_from_db(
    kind: NameKind,
    current: &str,
    database: &Path,
) -> Vec<CompletionCandidate> {
    let Ok(db) = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let sql = match kind {
        NameKind::Profile => "SELECT name FROM profiles ORDER BY name",
        NameKind::Credential => "SELECT name FROM credentials ORDER BY name",
        NameKind::Forward => "SELECT name FROM forwards ORDER BY name",
        NameKind::Group => "SELECT name FROM groups ORDER BY sort_order, name",
    };
    let Ok(mut statement) = db.prepare(sql) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| row.get(0)) else {
        return Vec::new();
    };
    let names = rows.filter_map(Result::ok).collect();

    candidates(names, current)
}

fn candidates(names: Vec<String>, current: &str) -> Vec<CompletionCandidate> {
    let current = current.to_ascii_lowercase();
    names
        .into_iter()
        .filter(|name| name.to_ascii_lowercase().starts_with(&current))
        .map(CompletionCandidate::new)
        .collect()
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[cfg(any(macos, linux))]
    fn check_zsh_registration(autoload: bool) {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("_rssh");
        let mut output = Vec::new();
        write_completions("zsh", &mut output).unwrap();
        std::fs::write(&script, output).unwrap();

        // Exercise zsh's real compinit/autoload dispatch. Stub the candidate
        // provider and renderer so this runs without ZLE, a database, or an
        // installed rssh executable.
        let result = std::process::Command::new("zsh")
            .args([
                "-fc",
                r#"
fpath=("$1" $fpath)
autoload -Uz compinit
compinit -D
rssh() {
    [[ $_RSSH_COMPLETE == zsh && $_CLAP_COMPLETE_INDEX == 1 ]] || return 1
    print -r -- 'profile:Manage SSH profiles'
}
_describe() { print -r -- "candidate:${other[1]}"; }
if [[ $2 == source ]]; then
    source "$1/_rssh"
fi
print -r -- ready
words=(rssh pr)
CURRENT=2
${_comps[rssh]}
"#,
                "completion-test",
            ])
            .arg(directory.path())
            .arg(if autoload { "autoload" } else { "source" })
            .output();
        let result = match result {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                eprintln!("Skipping zsh registration test: zsh is not installed");
                return;
            }
            other => other.unwrap(),
        };
        assert!(
            result.status.success(),
            "zsh failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            String::from_utf8(result.stdout).unwrap(),
            "ready\ncandidate:profile:Manage SSH profiles\n"
        );
    }

    #[test]
    #[cfg(any(macos, linux))]
    fn zsh_autoload_completes_on_first_call() {
        check_zsh_registration(true);
    }

    #[test]
    #[cfg(any(macos, linux))]
    fn zsh_source_registers_without_attempting_completion() {
        check_zsh_registration(false);
    }

    #[test]
    fn candidates_filter_by_current_prefix() {
        let values = candidates(
            vec!["Production".into(), "Proxy host".into(), "Staging".into()],
            "pro",
        );
        let values: Vec<_> = values
            .iter()
            .map(|candidate| candidate.get_value().to_string_lossy())
            .collect();

        assert_eq!(values, ["Production", "Proxy host"]);
    }

    #[test]
    fn candidates_include_every_name_for_empty_prefix() {
        let values = candidates(vec!["alpha".into(), "name with spaces".into()], "");

        assert_eq!(values.len(), 2);
        assert_eq!(values[1].get_value(), OsStr::new("name with spaces"));
    }

    #[test]
    fn database_candidates_are_read_only() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("rssh.db");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE profiles (name TEXT NOT NULL);\
                 INSERT INTO profiles VALUES ('Production'), ('Staging');",
            )
            .unwrap();
        drop(connection);

        let values = complete_names_from_db(NameKind::Profile, "prod", &database);
        assert_eq!(values[0].get_value(), OsStr::new("Production"));

        let missing = directory.path().join("missing.db");
        assert!(complete_names_from_db(NameKind::Profile, "", &missing).is_empty());
        assert!(!missing.exists());
    }

    #[test]
    fn every_named_resource_argument_has_a_completer() {
        let command = crate::Cli::command();
        for (resource, actions) in [
            ("profile", &["open", "edit", "rm"][..]),
            ("credential", &["edit", "rm"][..]),
            ("forward", &["open", "edit", "rm"][..]),
            ("group", &["edit", "rm"][..]),
        ] {
            let resource = command
                .find_subcommand(resource)
                .unwrap_or_else(|| panic!("missing {resource} command"));
            for action in actions {
                let argument = resource
                    .find_subcommand(action)
                    .unwrap_or_else(|| panic!("missing {action} command"))
                    .get_arguments()
                    .find(|argument| argument.get_id() == "name")
                    .unwrap_or_else(|| panic!("missing name argument for {action}"));
                assert!(argument.get::<clap_complete::ArgValueCompleter>().is_some());
            }
        }
    }
}
