//! File transfer from connector-backed terminals, without terminal output framing.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::ai::shell::ShellKind;
use crate::commands::discovery::{resolve_dynamic_discovery_program, ResolvedDiscoveryProgram};
use crate::error::{AppError, AppResult};
use crate::files::atomic::AtomicDownload;
use crate::models::{ConnectorSpec, DynamicDiscoveryPlatform};

fn is_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

/// Interpret the source's separators, then require a single host-side filename.
pub(super) fn source_basename(path: &str) -> Option<&str> {
    let windows = is_windows_absolute(path);
    let path = if let Some(unc) = path.strip_prefix("\\\\") {
        // A UNC server/share pair is the root, not a filename.
        let (_, share) = unc.split_once(['\\', '/'])?;
        let (_, relative) = share.split_once(['\\', '/'])?;
        relative
    } else {
        path
    };
    let name = if windows {
        path.rsplit(['\\', '/']).next()?
    } else {
        path.rsplit('/').next()?
    };
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(_)), None) => Some(name),
        _ => None,
    }
}

pub(super) fn connector_for_target(
    state: &crate::state::AppState,
    target_id: &str,
) -> AppResult<Option<ConnectorSpec>> {
    #[cfg(any(windows, macos, linux, ohos))]
    {
        let sessions = crate::error::locked(&state.pty_sessions)?;
        let handle = sessions
            .get(target_id)
            .ok_or_else(|| AppError::not_found("local_pty_not_found", json!({})))?;
        Ok(handle.connector().cloned())
    }
    #[cfg(any(android, ios))]
    {
        let _ = (state, target_id);
        Err(AppError::not_found("local_pty_not_found", json!({})))
    }
}

fn connector_command(
    spec: &ConnectorSpec,
    path: &str,
) -> AppResult<(DynamicDiscoveryPlatform, Vec<String>)> {
    // The host may use a different OS from the container. Do not interpret a
    // container path using std::path's host-specific absolute-path predicate.
    let absolute = path.starts_with('/') || is_windows_absolute(path);
    if !absolute {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Connector downloads require an absolute source path",
        )
        .into());
    }
    let (platform, mut args, shell) = match spec {
        ConnectorSpec::DockerExec {
            context,
            container_id,
            shell,
            ..
        } => (
            DynamicDiscoveryPlatform::Docker,
            vec![
                "--context".into(),
                context.clone(),
                "exec".into(),
                "--".into(),
                container_id.clone(),
            ],
            shell,
        ),
        ConnectorSpec::KubectlExec {
            context,
            namespace,
            pod,
            container,
            shell,
        } => {
            // A pod name comes from discovery, but still cannot become a flag.
            if pod.starts_with('-') || pod.is_empty() {
                return Err(AppError::config(
                    "dynamic_discovery_connector_invalid",
                    json!({}),
                ));
            }
            let mut args = vec![
                "--context".into(),
                context.clone(),
                "exec".into(),
                "--namespace".into(),
                namespace.clone(),
                pod.clone(),
            ];
            if let Some(container) = container.as_ref().filter(|name| !name.trim().is_empty()) {
                args.extend(["--container".into(), container.clone()]);
            }
            args.push("--".into());
            (DynamicDiscoveryPlatform::K8s, args, shell)
        }
        ConnectorSpec::AwsSsm { .. } => {
            return Err(AppError::other(
                "dynamic_discovery_aws_ssm_download_unsupported",
                json!({}),
            ));
        }
    };
    // No TTY or terminal sentinels. The native reader writes only file bytes
    // to stdout, and source paths never become executable shell text.
    args.extend(file_read_command(shell, path));
    Ok((platform, args))
}

fn file_read_command(shell: &str, path: &str) -> Vec<String> {
    let program = match ShellKind::from_local_path(shell) {
        ShellKind::Powershell => shell,
        ShellKind::Cmd => "powershell.exe",
        _ => return vec!["cat".into(), "--".into(), path.into()],
    };
    let path = STANDARD.encode(path.as_bytes());
    let script = format!(
        "$ErrorActionPreference='Stop'; $source=$null; \
         try {{ \
           $path=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{path}')); \
           $source=[IO.File]::OpenRead($path); \
           $source.CopyTo([Console]::OpenStandardOutput()); \
         }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 1 }} \
         finally {{ if ($null -ne $source) {{ $source.Dispose() }} }}; exit 0"
    );
    let encoded = STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    vec![
        program.into(),
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-EncodedCommand".into(),
        encoded,
    ]
}

pub(super) async fn copy_bounded<R: AsyncRead + Unpin>(
    source: &mut R,
    target: &mut AtomicDownload,
    path: &str,
    max_bytes: u64,
) -> AppResult<u64> {
    let mut transferred = 0;
    let mut buffer = vec![0u8; 32_768];
    loop {
        let count = source.read(&mut buffer).await?;
        if count == 0 {
            return Ok(transferred);
        }
        let next = transferred + count as u64;
        if next > max_bytes {
            return Err(AppError::other(
                "ai_download_file_too_large",
                json!({ "path": path, "size": next, "limit": max_bytes }),
            ));
        }
        target.stream().write_all(&buffer[..count]).await?;
        transferred = next;
    }
}

async fn read_stderr(mut source: impl AsyncRead + Unpin) -> AppResult<String> {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = source.read(&mut buffer).await?;
        if count == 0 {
            return Ok(String::from_utf8_lossy(&kept).trim().to_owned());
        }
        // Drain everything to prevent a full stderr pipe from blocking stdout,
        // while retaining only a bounded error message.
        let keep = count.min(16_384usize.saturating_sub(kept.len()));
        kept.extend_from_slice(&buffer[..keep]);
    }
}

pub(super) async fn copy_connector(
    spec: &ConnectorSpec,
    source: &str,
    destination: &Path,
    max_bytes: u64,
) -> AppResult<u64> {
    let (platform, args) = connector_command(spec, source)?;
    let program = resolve_dynamic_discovery_program(platform)?;
    copy_connector_with_program(
        program,
        &args,
        source,
        destination,
        max_bytes,
        Duration::from_secs(60),
    )
    .await
}

async fn copy_connector_with_program(
    program: ResolvedDiscoveryProgram,
    args: &[String],
    source: &str,
    destination: &Path,
    max_bytes: u64,
    timeout: Duration,
) -> AppResult<u64> {
    let mut child = tokio::process::Command::new(&program.executable)
        .env("PATH", program.search_path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdout = child.stdout.take().expect("download stdout is piped");
    let stderr = child.stderr.take().expect("download stderr is piped");
    let mut target = AtomicDownload::create(destination)?;
    let result = tokio::time::timeout(timeout, async {
        let (bytes, stderr) = tokio::try_join!(
            copy_bounded(&mut stdout, &mut target, source, max_bytes),
            read_stderr(stderr),
        )?;
        let status = child.wait().await?;
        if !status.success() {
            return Err(AppError::other(
                "dynamic_discovery_command_failed",
                json!({ "program": program.executable, "status": status.code(), "err": stderr }),
            ));
        }
        Ok(bytes)
    })
    .await
    .unwrap_or_else(|_| {
        Err(AppError::other(
            "dynamic_discovery_timeout",
            json!({ "program": program.executable, "secs": timeout.as_secs() }),
        ))
    });
    match result {
        Ok(bytes) => {
            target.commit().await?;
            Ok(bytes)
        }
        Err(error) => {
            let _ = child.kill().await;
            target.abort().await;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_basename_uses_windows_path_syntax_on_any_host() {
        let long_path = format!("C:\\{}\\artifact.log", "目录".repeat(45));
        assert!(long_path.len() > 255);
        for path in [
            long_path.as_str(),
            r"C:\Users\operator\artifact.log",
            "C:/Users/operator/artifact.log",
            r"\\server\share\directory\artifact.log",
            r"\\server\share\directory/artifact.log",
        ] {
            assert_eq!(source_basename(path), Some("artifact.log"), "{path}");
        }
    }

    #[test]
    fn source_basename_keeps_posix_names_and_rejects_roots_and_parent_components() {
        assert_eq!(source_basename("/tmp/诊断.log"), Some("诊断.log"));
        assert_eq!(source_basename("relative.log"), Some("relative.log"));
        #[cfg(not(windows))]
        {
            assert_eq!(
                source_basename(r"/tmp/literal\name.log"),
                Some(r"literal\name.log")
            );
            assert_eq!(
                source_basename(r"literal\name.log"),
                Some(r"literal\name.log")
            );
        }
        for path in [
            "",
            "/",
            ".",
            "..",
            "/tmp/..",
            r"C:\",
            "C:/",
            r"C:\tmp\..",
            r"C:\tmp\.",
            r"\\server\share",
            r"\\server\share\",
            r"\\server\share\..",
        ] {
            assert_eq!(source_basename(path), None, "{path}");
        }
        #[cfg(windows)]
        for path in ["/tmp/C:relative.log", r"/tmp/literal\name.log"] {
            assert_eq!(source_basename(path), None, "{path}");
        }
    }

    fn docker() -> ConnectorSpec {
        ConnectorSpec::DockerExec {
            context: "remote-context".into(),
            container_id: "container-id".into(),
            container_name: "container-name".into(),
            shell: "sh".into(),
        }
    }

    #[tokio::test]
    async fn aws_ssm_download_never_reads_the_same_path_on_the_host() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("local-secret");
        let destination = directory.path().join("artifact");
        std::fs::write(&source, b"host-only bytes").unwrap();
        std::fs::write(&destination, b"existing artifact").unwrap();
        let spec = ConnectorSpec::AwsSsm {
            profile: String::new(),
            region: "us-east-1".into(),
            instance_id: "i-0123456789abcdef0".into(),
            account_id: "123456789012".into(),
            shell: "sh".into(),
        };
        let error = copy_connector(&spec, source.to_str().unwrap(), &destination, 1024)
            .await
            .unwrap_err();
        assert_eq!(
            error.code(),
            "dynamic_discovery_aws_ssm_download_unsupported"
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"existing artifact");
    }

    #[test]
    fn connector_download_arguments_preserve_context_and_literal_path() {
        let path = "/tmp/中文 '$(touch should-not-run); file.log";
        let (platform, args) = connector_command(&docker(), path).unwrap();
        assert_eq!(platform, DynamicDiscoveryPlatform::Docker);
        assert_eq!(
            args,
            [
                "--context",
                "remote-context",
                "exec",
                "--",
                "container-id",
                "cat",
                "--",
                path
            ]
        );
        let k8s = ConnectorSpec::KubectlExec {
            context: "cluster".into(),
            namespace: "team".into(),
            pod: "pod-name".into(),
            container: Some("application".into()),
            shell: "sh".into(),
        };
        let (platform, args) = connector_command(&k8s, path).unwrap();
        assert_eq!(platform, DynamicDiscoveryPlatform::K8s);
        assert_eq!(
            args,
            [
                "--context",
                "cluster",
                "exec",
                "--namespace",
                "team",
                "pod-name",
                "--container",
                "application",
                "--",
                "cat",
                "--",
                path
            ]
        );
        assert!(connector_command(&docker(), "relative.log").is_err());
    }

    #[test]
    fn connector_download_selects_the_native_file_reader() {
        for (shell, expected_program) in [
            ("/bin/sh", "cat"),
            ("pwsh", "pwsh"),
            (
                r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
                r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
            ),
            ("cmd.exe", "powershell.exe"),
        ] {
            let mut spec = docker();
            let ConnectorSpec::DockerExec {
                shell: target_shell,
                ..
            } = &mut spec
            else {
                unreachable!()
            };
            *target_shell = shell.into();
            let (_, args) =
                connector_command(&spec, r"C:\中文 '$(Write-Output BAD); file.bin").unwrap();
            assert_eq!(args[5], expected_program);
            if expected_program != "cat" {
                assert_eq!(
                    &args[6..9],
                    ["-NoProfile", "-NonInteractive", "-EncodedCommand"]
                );
                assert_eq!(args.len(), 10);
            }
        }
    }

    #[tokio::test]
    async fn powershell_connector_download_preserves_binary_bytes_and_literal_unicode_paths() {
        if !std::process::Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-Command", "exit 0"])
            .output()
            .is_ok_and(|output| output.status.success())
        {
            eprintln!("pwsh unavailable; skipping PowerShell download execution test");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("诊断 '$(Write-Output BAD); [源].bin");
        let destination = directory.path().join("copied.bin");
        let content = [0, 255, 254, 128, 195, 40, 13, 10, 0, 27, 65];
        std::fs::write(&source, content).unwrap();
        let mut spec = docker();
        let ConnectorSpec::DockerExec { shell, .. } = &mut spec else {
            unreachable!()
        };
        *shell = "pwsh".into();
        let source_path = source.to_str().unwrap();
        let (_, args) = connector_command(&spec, source_path).unwrap();
        let program = || ResolvedDiscoveryProgram {
            executable: "pwsh".into(),
            search_path: std::env::var_os("PATH").unwrap_or_default(),
        };
        let bytes = copy_connector_with_program(
            program(),
            &args[6..],
            source_path,
            &destination,
            4096,
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(bytes, content.len() as u64);
        assert_eq!(std::fs::read(&destination).unwrap(), content);

        std::fs::remove_file(&source).unwrap();
        let error = copy_connector_with_program(
            program(),
            &args[6..],
            source_path,
            &destination,
            4096,
            Duration::from_secs(10),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "dynamic_discovery_command_failed");
        assert_eq!(std::fs::read(&destination).unwrap(), content);
    }

    #[cfg(unix)]
    fn program(script: &str, args: &[String]) -> (ResolvedDiscoveryProgram, Vec<String>) {
        // A concurrent fork can inherit a temporary executable's write handle,
        // causing ETXTBSY on Linux even after this thread closes it. Use the
        // installed interpreter, with connector arguments kept separate from
        // the fixed fixture script so their contents are never evaluated.
        let program = ResolvedDiscoveryProgram {
            executable: "/bin/sh".into(),
            search_path: std::env::var_os("PATH").unwrap_or_default(),
        };
        let mut script_args = vec!["-c".into(), script.into(), "fake-cli".into()];
        script_args.extend_from_slice(args);
        (program, script_args)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connector_download_preserves_literal_argv_and_commits_exact_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("中文.log");
        let path = "/tmp/$(printf injected); file.log";
        let (_, args) = connector_command(&docker(), path).unwrap();
        let expected = format!("{}\n", args.join("\n"));
        let (program, args) = program("printf '%s\\n' \"$@\"", &args);
        let result = copy_connector_with_program(
            program,
            &args,
            path,
            &destination,
            4096,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(result, expected.len() as u64);
        assert_eq!(std::fs::read(destination).unwrap(), expected.as_bytes());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connector_download_failure_size_limit_and_timeout_keep_the_existing_artifact() {
        for (script, max_bytes, timeout, code) in [
            (
                "printf oversized",
                3,
                Duration::from_secs(2),
                "ai_download_file_too_large",
            ),
            (
                "printf partial; printf denied >&2; exit 7",
                4096,
                Duration::from_secs(2),
                "dynamic_discovery_command_failed",
            ),
            (
                "printf partial; exec sleep 30",
                4096,
                Duration::from_millis(30),
                "dynamic_discovery_timeout",
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let destination = directory.path().join("existing.log");
            std::fs::write(&destination, b"original").unwrap();
            let (program, args) = program(script, &[]);
            let error = copy_connector_with_program(
                program,
                &args,
                "/container.log",
                &destination,
                max_bytes,
                timeout,
            )
            .await
            .unwrap_err();
            assert_eq!(error.code(), code);
            assert_eq!(std::fs::read(destination).unwrap(), b"original");
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_connector_download_removes_the_partial_file() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("existing.log");
        std::fs::write(&destination, b"original").unwrap();
        let (program, args) = program("printf partial; exec sleep 30", &[]);
        let target = destination.clone();
        let task = tokio::spawn(async move {
            copy_connector_with_program(
                program,
                &args,
                "/container.log",
                &target,
                4096,
                Duration::from_secs(60),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while std::fs::read_dir(directory.path()).unwrap().count() < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(2), async {
            while std::fs::read_dir(directory.path()).unwrap().count() > 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(destination).unwrap(), b"original");
    }
}
