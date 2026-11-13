#[cfg(any(feature = "tauri", test))]
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;
#[cfg(feature = "tauri")]
use tauri::{AppHandle, Manager};

#[cfg(feature = "tauri")]
use crate::error::{AppError, AppResult};

use rssh_core::CLI_VERSION;

#[derive(Serialize)]
pub struct CliStatus {
    pub installed: bool,
    pub path: String,
    pub bundled: bool,
    pub installed_version: Option<String>,
    pub expected_version: &'static str,
    pub needs_update: bool,
}

fn install_dir() -> PathBuf {
    if cfg!(windows) {
        dirs::data_local_dir()
            .unwrap_or_default()
            .join("Programs")
            .join("rssh")
    } else {
        PathBuf::from("/usr/local/bin")
    }
}

fn cli_name() -> &'static str {
    if cfg!(windows) {
        "rssh.exe"
    } else {
        "rssh"
    }
}

fn find_installed() -> Option<PathBuf> {
    let name = cli_name();
    // Check common paths
    let candidates = if cfg!(windows) {
        vec![
            install_dir().join(name),
            dirs::home_dir()
                .unwrap_or_default()
                .join(".cargo")
                .join("bin")
                .join(name),
        ]
    } else {
        vec![
            PathBuf::from("/usr/local/bin").join(name),
            dirs::home_dir()
                .unwrap_or_default()
                .join(".local")
                .join("bin")
                .join(name),
            dirs::home_dir()
                .unwrap_or_default()
                .join(".cargo")
                .join("bin")
                .join("rssh-cli"),
        ]
    };
    candidates.into_iter().find(|p| p.exists())
}

#[cfg(feature = "tauri")]
fn find_bundled(app: &AppHandle) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "rssh-cli.exe"
    } else {
        "rssh-cli"
    };

    // 1. Production: bundled in app resources
    if let Ok(dir) = app.path().resource_dir() {
        let p = dir.join("bin").join(name);
        if p.exists() {
            return Some(p);
        }
    }

    // 2. Dev: local cargo build output
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("release")
        .join(name);
    if dev.exists() {
        return Some(dev);
    }

    None
}

fn installed_version(path: &PathBuf) -> Option<String> {
    let output = Command::new(path).arg("version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?.trim().to_string();
    parse_version(&version).map(|_| version)
}

fn parse_version(version: &str) -> Option<Vec<u64>> {
    let version = version.strip_prefix('v').unwrap_or(version);
    if version.is_empty() {
        return None;
    }
    version.split('.').map(|part| part.parse().ok()).collect()
}

fn version_is_older(installed: &str, expected: &str) -> bool {
    let Some(mut installed) = parse_version(installed) else {
        return true;
    };
    let Some(mut expected) = parse_version(expected) else {
        return false;
    };
    let length = installed.len().max(expected.len());
    installed.resize(length, 0);
    expected.resize(length, 0);
    installed < expected
}

fn build_status(installed: Option<PathBuf>, bundled: bool) -> CliStatus {
    let installed_version = installed.as_ref().and_then(installed_version);
    let needs_update = installed_version
        .as_deref()
        .map(|version| version_is_older(version, CLI_VERSION))
        .unwrap_or(true);

    CliStatus {
        installed: installed.is_some(),
        path: installed
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        bundled,
        installed_version,
        expected_version: CLI_VERSION,
        needs_update,
    }
}

#[cfg(feature = "tauri")]
pub fn cli_status(app: AppHandle) -> CliStatus {
    build_status(find_installed(), find_bundled(&app).is_some())
}

/// Headless CLI status: PATH-based install check only. The bundled-resource
/// probe needs a Tauri `AppHandle`, which the headless server doesn't have;
/// the embedded server prepends its own dir to the shell PATH instead, so it
/// reports `bundled: false` and leaves install to the host (IDEA plugin / app).
#[cfg(feature = "server")]
pub fn cli_status_headless() -> CliStatus {
    build_status(find_installed(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison_handles_old_equal_and_newer_cli() {
        assert!(version_is_older("0.9.9", CLI_VERSION));
        assert!(!version_is_older(CLI_VERSION, CLI_VERSION));
        assert!(!version_is_older("1.1.0", CLI_VERSION));
        assert!(version_is_older("1.0.0", CLI_VERSION));
        assert!(!version_is_older("1.0", "1.0.0"));
    }

    #[test]
    fn malformed_version_is_outdated() {
        assert!(version_is_older("rssh 1.0.0", CLI_VERSION));
        assert!(version_is_older("", CLI_VERSION));
    }

    #[test]
    fn zsh_install_preserves_config_and_refreshes_script_without_duplicate_init() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("custom-zdotdir");
        std::fs::create_dir(&config).unwrap();
        let existing = "# user's config\nexport EDITOR=vim";
        std::fs::write(config.join(".zshrc"), existing).unwrap();

        install_zsh_completions(home.path(), &config, b"first script").unwrap();
        let first = std::fs::read_to_string(config.join(".zshrc")).unwrap();
        install_zsh_completions(home.path(), &config, b"updated script").unwrap();

        assert!(first.starts_with(existing));
        assert_eq!(
            std::fs::read_to_string(config.join(".zshrc")).unwrap(),
            first
        );
        assert!(!home.path().join(".zshrc").exists());
        assert_eq!(
            std::fs::read(home.path().join(".zsh/completions/_rssh")).unwrap(),
            b"updated script"
        );
    }

    #[cfg(any(macos, linux))]
    #[test]
    fn zsh_install_registers_completion_in_a_fresh_shell_and_preserves_existing_compinit() {
        match Command::new("zsh").arg("--version").output() {
            Ok(output) => assert!(output.status.success()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => panic!("failed to run zsh: {error}"),
        }
        let home = tempfile::tempdir().unwrap();
        install_zsh_completions(
            home.path(),
            home.path(),
            b"_rssh_test() { :; }\ncompdef _rssh_test rssh\n",
        )
        .unwrap();
        let output = Command::new("zsh")
            .args([
                "-f",
                "-c",
                r#"# Exclude unrelated site-functions from the runner's fpath while
# retaining the real compinit/compaudit and their normal permission checks.
fpath=( ${^fpath}/compinit(N:h) )
(( $#fpath )) || exit 1
source "$HOME/.zshrc"
[[ ${_comps[rssh]} == _rssh_test ]] || exit 1
compinit() { print -u2 'compinit must not run twice'; return 1; }
source "$HOME/.zshrc"
[[ ${_comps[rssh]} == _rssh_test ]]"#,
            ])
            .env("HOME", home.path())
            .env("ZDOTDIR", home.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        assert!(output.stderr.is_empty(), "{:?}", output);
    }

    #[test]
    fn zsh_install_reports_unreadable_config_instead_of_replacing_it() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".zshrc")).unwrap();
        assert!(install_zsh_completions(home.path(), home.path(), b"script").is_err());
        assert!(home.path().join(".zshrc").is_dir());
    }

    #[cfg(any(macos, linux))]
    #[test]
    fn zsh_install_discovers_zdotdir_from_zshenv_without_loading_zshrc() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("custom zsh config");
        std::fs::create_dir(&config).unwrap();
        std::fs::write(
            home.path().join(".zshenv"),
            "print -r -- 'startup output'\nprint -u2 -- 'startup diagnostic'\nexport ZDOTDIR=\"$HOME/custom zsh config\"\n",
        )
        .unwrap();
        std::fs::write(config.join(".zshrc"), "return 42\n").unwrap();

        let detected = match zsh_config_dir("zsh", home.path(), None) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            result => result.unwrap(),
        };
        assert_eq!(detected, config);
        install_zsh_completions(home.path(), &detected, b"completion script").unwrap();
        assert!(!home.path().join(".zshrc").exists());
        assert!(std::fs::read_to_string(config.join(".zshrc"))
            .unwrap()
            .contains(ZSH_COMPLETION_INIT));
    }

    #[cfg(any(macos, linux))]
    #[test]
    fn zsh_config_probe_preserves_startup_errors() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(".zshenv"),
            "print -u2 -- 'startup failed'\nexit 17\n",
        )
        .unwrap();
        let error = zsh_config_dir("zsh", home.path(), None).unwrap_err();
        if error.kind() == std::io::ErrorKind::NotFound {
            return;
        }
        assert!(error.to_string().contains("startup failed"));
        assert!(error.to_string().contains("17"));
    }
}

#[cfg(feature = "tauri")]
pub fn cli_install(app: AppHandle) -> AppResult<String> {
    let src = find_bundled(&app)
        .ok_or_else(|| AppError::other("cli_not_bundled", serde_json::json!({})))?;

    let dest_dir = install_dir();
    let dest = dest_dir.join(cli_name());

    #[cfg(macos)]
    {
        let script = format!(
            r#"do shell script "mkdir -p '{}' && cp '{}' '{}' && chmod 755 '{}'" with administrator privileges"#,
            dest_dir.display(),
            src.display(),
            dest.display(),
            dest.display()
        );
        let status = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .status()
            .map_err(|e| {
                AppError::other(
                    "cli_osascript_failed",
                    serde_json::json!({ "err": e.to_string() }),
                )
            })?;
        if !status.success() {
            return Err(AppError::other(
                "cli_install_cancelled",
                serde_json::json!({}),
            ));
        }
    }

    #[cfg(linux)]
    {
        let status = Command::new("pkexec")
            .arg("sh")
            .arg("-c")
            .arg(format!(
                "mkdir -p '{}' && cp '{}' '{}' && chmod 755 '{}'",
                dest_dir.display(),
                src.display(),
                dest.display(),
                dest.display()
            ))
            .status()
            .map_err(|e| {
                AppError::other(
                    "cli_priv_request_failed",
                    serde_json::json!({ "err": e.to_string() }),
                )
            })?;
        if !status.success() {
            return Err(AppError::other(
                "cli_install_cancelled",
                serde_json::json!({}),
            ));
        }
    }

    #[cfg(windows)]
    {
        std::fs::create_dir_all(&dest_dir)?;
        std::fs::copy(&src, &dest)?;
        // Add to user PATH via PowerShell
        let _ = Command::new("powershell").arg("-Command")
            .arg(format!(
                r#"$p = [Environment]::GetEnvironmentVariable('Path','User'); if ($p -notlike '*{}*') {{ [Environment]::SetEnvironmentVariable('Path', $p + ';{}', 'User') }}"#,
                dest_dir.display(), dest_dir.display()
            ))
            .status();
    }

    // Setup completions (user-level, no admin needed)
    setup_completions(&dest)?;

    Ok(dest.display().to_string())
}

#[cfg(feature = "tauri")]
fn setup_completions(cli: &PathBuf) -> AppResult<()> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Ok(()),
    };

    let shell = std::env::var("SHELL").unwrap_or_default();

    if shell.contains("zsh") {
        let out = Command::new(cli).arg("completions").arg("zsh").output()?;
        if !out.status.success() || out.stdout.is_empty() {
            return Err(AppError::other(
                "cli_completions_failed",
                serde_json::json!({ "err": String::from_utf8_lossy(&out.stderr).trim() }),
            ));
        }
        let config_dir = zsh_config_dir(&shell, &home, std::env::var_os("ZDOTDIR").as_deref())?;
        install_zsh_completions(&home, &config_dir, &out.stdout)?;
    } else if shell.contains("bash") {
        let dir = home
            .join(".local")
            .join("share")
            .join("bash-completion")
            .join("completions");
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(out) = Command::new(cli).arg("completions").arg("bash").output() {
            let _ = std::fs::write(dir.join("rssh"), &out.stdout);
        }
    } else if shell.contains("fish") {
        let dir = home.join(".config").join("fish").join("completions");
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(out) = Command::new(cli).arg("completions").arg("fish").output() {
            let _ = std::fs::write(dir.join("rssh.fish"), &out.stdout);
        }
    }

    #[cfg(windows)]
    {
        // PowerShell completion — append to profile if not already present
        if let Ok(out) = Command::new(cli)
            .arg("completions")
            .arg("powershell")
            .output()
        {
            let profile = home
                .join("Documents")
                .join("PowerShell")
                .join("Microsoft.PowerShell_profile.ps1");
            let _ = std::fs::create_dir_all(profile.parent().unwrap());
            let existing = std::fs::read_to_string(&profile).unwrap_or_default();
            if !existing.contains("Register-ArgumentCompleter -Native -CommandName rssh") {
                let mut content = existing;
                content.push_str("\n# RSSH completions\n");
                content.push_str(&String::from_utf8_lossy(&out.stdout));
                let _ = std::fs::write(&profile, content);
            }
        }
    }

    Ok(())
}

// Source the registration after compinit, even when another framework already
// initialized completion. Merely writing an _rssh file does not load it in zsh.
#[cfg(any(feature = "tauri", test))]
const ZSH_COMPLETION_INIT: &str = r#"# >>> RSSH completions >>>
if [[ -r "$HOME/.zsh/completions/_rssh" ]]; then
    if (( ! $+functions[compdef] )); then
        autoload -Uz compinit
        compinit
    fi
    source "$HOME/.zsh/completions/_rssh"
fi
# <<< RSSH completions <<<
"#;

#[cfg(any(feature = "tauri", test))]
fn zsh_config_dir(
    shell: &str,
    home: &Path,
    inherited_zdotdir: Option<&std::ffi::OsStr>,
) -> std::io::Result<PathBuf> {
    // GUI launches need not inherit ZDOTDIR. Let zsh evaluate .zshenv, without
    // loading interactive .zshrc, and frame the result apart from startup output.
    let mut command = Command::new(shell);
    command
        .args([
            "-c",
            r#"rssh_completion_dir="${ZDOTDIR-$HOME}/"
builtin printf '\0RSSH_ZDOTDIR\0%s\0' "${rssh_completion_dir:a}""#,
        ])
        .env("HOME", home)
        .env_remove("ZDOTDIR");
    if let Some(zdotdir) = inherited_zdotdir {
        command.env("ZDOTDIR", zdotdir);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "zsh configuration probe ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    if !output.stderr.is_empty() {
        log::warn!(
            "zsh configuration probe: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    const MARKER: &[u8] = b"\0RSSH_ZDOTDIR\0";
    let directory = output
        .stdout
        .windows(MARKER.len())
        .position(|value| value == MARKER)
        .and_then(|start| {
            let value = &output.stdout[start + MARKER.len()..];
            value
                .iter()
                .position(|byte| *byte == 0)
                .map(|end| &value[..end])
        })
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "zsh did not return its configuration directory",
            )
        })?;
    let directory = std::str::from_utf8(directory)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(PathBuf::from(directory))
}

#[cfg(any(feature = "tauri", test))]
fn install_zsh_completions(home: &Path, config_dir: &Path, script: &[u8]) -> std::io::Result<()> {
    let dir = home.join(".zsh/completions");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("_rssh"), script)?;

    let rc = config_dir.join(".zshrc");
    let mut content = match std::fs::read_to_string(&rc) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    if !content.contains("# >>> RSSH completions >>>") {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push('\n');
        content.push_str(ZSH_COMPLETION_INIT);
        std::fs::create_dir_all(config_dir)?;
        std::fs::write(rc, content)?;
    }
    Ok(())
}
