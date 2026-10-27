/// Header availability does not establish permission to launch a shell. Probe
/// openpty + spawn + a successful child exit in this application's sandbox.
pub async fn local_pty_available() -> bool {
    static AVAILABLE: tokio::sync::OnceCell<bool> = tokio::sync::OnceCell::const_new();
    *AVAILABLE
        .get_or_init(|| async {
            tokio::task::spawn_blocking(probe_local_pty)
                .await
                .unwrap_or_else(|error| {
                    log::warn!("local PTY capability probe failed: {error}");
                    false
                })
        })
        .await
}

fn probe_local_pty() -> bool {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    let Some(shell) = rssh_core::terminal::pty::available_shells()
        .into_iter()
        .next()
    else {
        log::info!("local PTY unavailable: no executable shell in the application sandbox");
        return false;
    };
    let pair = match native_pty_system().openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pair) => pair,
        Err(error) => {
            log::info!("local PTY unavailable: openpty failed: {error}");
            return false;
        }
    };
    let mut command = CommandBuilder::new(&shell);
    command.args(["-c", "exit 0"]);
    let mut child = match pair.slave.spawn_command(command) {
        Ok(child) => child,
        Err(error) => {
            log::info!("local PTY unavailable: spawning {shell} failed: {error}");
            return false;
        }
    };
    drop(pair.slave);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            result => {
                log::warn!("local PTY probe did not complete: {result:?}");
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}
