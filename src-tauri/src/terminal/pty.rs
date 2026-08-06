use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use crate::error::{locked, AppError, AppResult};

/// PTY output destined for the host. The Tauri command turns these into
/// `app.emit("pty:data:<id>")` / `pty:close:<id>`; the headless ws server
/// pushes them to its socket. `spawn` itself stays transport-agnostic.
pub enum PtyOut {
    Data(Vec<u8>),
    ShellForeground,
    Close,
}

/// Sink the reader thread invokes for each chunk. The `&str` is the session
/// id, so one sink can serve any number of PTY sessions.
pub type PtySink = Arc<dyn Fn(&str, PtyOut) + Send + Sync>;

#[cfg(unix)]
#[derive(Debug)]
struct ForegroundProcessState {
    shell_pgrp: libc::pid_t,
    foreground_pgrp: libc::pid_t,
    shell_return_pending: bool,
}

#[cfg(unix)]
impl ForegroundProcessState {
    fn new(shell_pgrp: libc::pid_t) -> Self {
        Self {
            shell_pgrp,
            foreground_pgrp: shell_pgrp,
            shell_return_pending: false,
        }
    }

    fn observe(&mut self, foreground_pgrp: libc::pid_t) -> bool {
        let returned_to_shell =
            self.foreground_pgrp != self.shell_pgrp && foreground_pgrp == self.shell_pgrp;
        if foreground_pgrp != self.shell_pgrp {
            self.shell_return_pending = false;
        } else if returned_to_shell {
            self.shell_return_pending = true;
        }
        self.foreground_pgrp = foreground_pgrp;
        returned_to_shell
    }

    fn take_stale_mouse_report(&mut self, data: &[u8]) -> bool {
        if !self.shell_return_pending || !is_xterm_mouse_report(data) {
            return false;
        }
        self.shell_return_pending = false;
        true
    }
}

#[cfg(unix)]
type ForegroundProcessTracker = Arc<Mutex<ForegroundProcessState>>;

#[cfg(unix)]
fn observe_foreground_process(
    master: &Arc<Mutex<Box<dyn MasterPty + Send>>>,
    tracker: Option<&ForegroundProcessTracker>,
) -> bool {
    let Some(tracker) = tracker else {
        return false;
    };
    let Some(pgrp) = master
        .lock()
        .ok()
        .and_then(|master| master.process_group_leader())
    else {
        return false;
    };
    tracker
        .lock()
        .map(|mut state| state.observe(pgrp))
        .unwrap_or(false)
}

#[cfg(unix)]
fn inspect_foreground_input(
    master: &Arc<Mutex<Box<dyn MasterPty + Send>>>,
    tracker: Option<&ForegroundProcessTracker>,
    data: &[u8],
) -> (bool, bool) {
    let Some(tracker) = tracker else {
        return (false, false);
    };
    let Some(pgrp) = master
        .lock()
        .ok()
        .and_then(|master| master.process_group_leader())
    else {
        return (false, false);
    };
    let Ok(mut state) = tracker.lock() else {
        return (false, false);
    };
    let returned_to_shell = state.observe(pgrp);
    let stale_mouse_report = state.take_stale_mouse_report(data);
    (returned_to_shell || stale_mouse_report, stale_mouse_report)
}

#[cfg(unix)]
fn is_xterm_mouse_report(data: &[u8]) -> bool {
    if data.len() == 6 && data.starts_with(b"\x1b[M") {
        return true;
    }
    if data.len() < 9 || !data.starts_with(b"\x1b[<") {
        return false;
    }
    if !matches!(data.last(), Some(b'M' | b'm')) {
        return false;
    }

    let mut params = data[3..data.len() - 1].split(|byte| *byte == b';');
    (0..3).all(|_| {
        params
            .next()
            .is_some_and(|param| !param.is_empty() && param.iter().all(u8::is_ascii_digit))
    }) && params.next().is_none()
}

/// 子进程持有者：保证 PtyHandle 最后一份 clone 被 drop 时（tab 关闭 / session 结束），
/// 显式 kill + wait 子 shell。否则 Box<dyn Child> 在 spawn() 返回后立刻 drop，
/// 子进程退出后无人 reap，留 zombie 占 PID。
/// `Box<dyn Child + Send>` 不带 `Sync`：portable_pty 的 Child 实现普遍只是
/// Send。`Mutex<T>` 自身在 `T: Send` 时即是 Sync，无需 inner 也 Sync——
/// 加多余的 Sync bound 在某些平台上会编不过。
struct ChildReaper {
    child: Mutex<Option<Box<dyn Child + Send>>>,
}

impl Drop for ChildReaper {
    fn drop(&mut self) {
        // Drop 在 Arc 计数归零时跑一次。kill + wait 通常 < 100ms（SIGKILL → 内核 reap）。
        if let Ok(mut g) = self.child.lock() {
            if let Some(mut c) = g.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

/// 本地 PTY 会话句柄，Clone + Send + Sync。
/// `_reaper` 跟着 PtyHandle 走，最后一份 clone 消失时回收子进程。
/// `shell_path` 是 spawn 时实际使用的 shell 二进制路径——AI session 用它
/// 推断 ShellKind（无需探测，因为本地 shell 是用户在 UI 里显式选的）。
#[derive(Clone)]
pub struct PtyHandle {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    shell_path: Arc<str>,
    #[cfg(unix)]
    session_id: Arc<str>,
    #[cfg(unix)]
    sink: PtySink,
    #[cfg(unix)]
    foreground_process: Option<ForegroundProcessTracker>,
    _reaper: Arc<ChildReaper>,
}

impl PtyHandle {
    pub fn write(&self, data: &[u8]) -> AppResult<()> {
        #[cfg(unix)]
        let (reset_mouse_tracking, stale_mouse_report) =
            inspect_foreground_input(&self.master, self.foreground_process.as_ref(), data);
        #[cfg(unix)]
        if reset_mouse_tracking {
            (self.sink)(&self.session_id, PtyOut::ShellForeground);
        }
        #[cfg(unix)]
        if stale_mouse_report {
            // This input was generated while xterm still believed the dead
            // foreground program owned mouse tracking. It belongs to neither
            // process, so do not hand it to the shell as printable text.
            return Ok(());
        }
        locked(&self.writer)?.write_all(data).map_err(|e| {
            AppError::pty("pty_op_failed", serde_json::json!({ "err": e.to_string() }))
        })
    }

    pub fn resize(&self, cols: u16, rows: u16) -> AppResult<()> {
        locked(&self.master)?
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| {
                AppError::pty("pty_op_failed", serde_json::json!({ "err": e.to_string() }))
            })
    }

    /// spawn 时实际使用的 shell 路径（用户在 UI 选的，或 default_shell 兜底）。
    /// AI 模块用这个判定本地 PTY 的 ShellKind，无需探测。
    pub fn shell_path(&self) -> &str {
        &self.shell_path
    }
}

/// 启动本地 shell，返回 (session_id, handle)。
/// 读取线程通过 Tauri 事件 `pty:data:{id}` 推送数据。

/// 本机实际可用的 shell 路径列表。启动时扫描一次进缓存；
/// 用户在 Shell 设置页点"刷新"会重扫覆盖（用户 `brew install fish`
/// 之类的中途变化得有补救手段，否则要 restart app 才看得到）。
/// RwLock 比 OnceLock 多支持一个 write 路径 —— 读路径几乎没有竞争开销。
/// `Option<Vec>` 区分"未初始化"和"扫出来空"两种状态：未初始化时 lazy 扫一次。
static AVAILABLE_SHELLS: std::sync::RwLock<Option<Vec<String>>> = std::sync::RwLock::new(None);

/// 启动时由 lib.rs 调一次预热。重复调跟 refresh 一样语义。
pub fn init_available_shells() {
    refresh_available_shells();
}

/// 重新扫描并覆盖缓存。Shell 设置页"刷新"按钮 / 用户装新 shell 后调。
pub fn refresh_available_shells() {
    let scanned = scan_shells();
    if let Ok(mut g) = AVAILABLE_SHELLS.write() {
        *g = Some(scanned);
    }
}

pub fn available_shells() -> Vec<String> {
    if let Ok(g) = AVAILABLE_SHELLS.read() {
        if let Some(v) = g.as_ref() {
            return v.clone();
        }
    }
    // 还没初始化（lib.rs 没调到 init，或调用方不是桌面端）—— lazy 扫一次。
    let scanned = scan_shells();
    if let Ok(mut g) = AVAILABLE_SHELLS.write() {
        *g = Some(scanned.clone());
    }
    scanned
}

/// 真正的"shell 候选"判据：必须是普通文件 + Unix 上有执行位。
/// 比 `Path::exists()` 严：能挡掉 `/etc/shells` / PATH 里的目录、破损 symlink、
/// 纯数据文件等乱入，避免最后 spawn 报"not executable"。
#[cfg(unix)]
fn is_shell_candidate(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
        .unwrap_or(false)
}

/// Windows 没有 POSIX 执行位，靠扩展名 + is_file。我们 KNOWN 列表里全是
/// `.exe` 后缀，普通文件即可。
#[cfg(windows)]
fn is_shell_candidate(path: &std::path::Path) -> bool {
    path.is_file()
}

/// 按 canonical path 去重：保留首次出现的字符串路径。
/// canonicalize 失败时（不存在 / 权限 / NixOS store 之类）回退原路径，
/// 退化为字符串去重，不丢东西。
#[cfg(any(unix, windows))]
fn dedup_by_canonical(paths: Vec<String>) -> Vec<String> {
    use std::collections::HashSet;
    use std::path::PathBuf;
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let canon = std::fs::canonicalize(&p).unwrap_or_else(|_| PathBuf::from(&p));
        if seen.insert(canon) {
            out.push(p);
        }
    }
    out
}

fn scan_shells() -> Vec<String> {
    #[cfg(unix)]
    {
        scan_unix()
    }
    #[cfg(windows)]
    {
        scan_windows()
    }
    #[cfg(not(any(unix, windows)))]
    {
        Vec::new()
    }
}

#[cfg(unix)]
fn scan_unix() -> Vec<String> {
    use std::path::Path;
    use std::path::PathBuf;

    // 收集所有候选 —— /etc/shells 优先（系统级权威清单）、PATH 扫描补漏、
    // $SHELL 兜底。中间不去重，最后走 canonical 去重一遍。
    let mut candidates: Vec<String> = Vec::new();

    // 1) /etc/shells —— 系统级权威清单（chsh -a / 包管理装 shell 都会写这里）。
    //    每行可能带 `#` 注释 + 空行 + 不存在路径（清单陈旧），全过滤掉。
    if let Ok(content) = std::fs::read_to_string("/etc/shells") {
        for line in content.lines() {
            let s = line.split('#').next().unwrap_or("").trim();
            if !s.is_empty() && is_shell_candidate(Path::new(s)) {
                candidates.push(s.to_string());
            }
        }
    }

    // 2) 在 PATH 里 which 一组已知 shell 名，捞漏。覆盖 `/etc/shells` 没注册的：
    //    - 用户 `cargo install nu` 没 `chsh -a`
    //    - Homebrew 装 fish 在 `/opt/homebrew/bin/fish`、`/usr/local/bin/fish`
    //    - 类 Termux / NixOS 这种 `/etc/shells` 不完整或不存在的环境
    const KNOWN_UNIX: &[&str] = &[
        "bash", "zsh", "fish", "dash", "sh", "ksh", "tcsh", "csh", "nu", "xonsh", "elvish", "ion",
        "pwsh",
    ];
    if let Ok(path_env) = std::env::var("PATH") {
        for dir in path_env.split(':').filter(|d| !d.is_empty()) {
            for name in KNOWN_UNIX {
                let candidate = format!("{dir}/{name}");
                if is_shell_candidate(Path::new(&candidate)) {
                    candidates.push(candidate);
                }
            }
        }
    }

    // 3) $SHELL 兜底 —— 上面两步可能都漏了用户自己手编译塞到 ~/bin 的 shell。
    let preferred = std::env::var("SHELL").ok();
    if let Some(s) = preferred.as_ref() {
        if is_shell_candidate(Path::new(s)) {
            candidates.push(s.clone());
        }
    }

    // canonical 去重：macOS 上 /bin/bash 和 /usr/bin/bash 是同一个 inode，
    // 字符串去重会留两个；canonicalize 之后用真身路径作 set key，只留一个。
    let mut shells = dedup_by_canonical(candidates);
    shells.sort();

    // $SHELL 排第一（用户偏好）。可能用户的 $SHELL 是 /bin/bash 但 dedup 留下
    // 的是 /usr/bin/bash —— 走 canonical 匹配，避免字符串比对漏掉。
    if let Some(pref) = preferred {
        let pref_canon = std::fs::canonicalize(&pref).unwrap_or_else(|_| PathBuf::from(&pref));
        if let Some(idx) = shells.iter().position(|s| {
            std::fs::canonicalize(s).unwrap_or_else(|_| PathBuf::from(s)) == pref_canon
        }) {
            let head = shells.remove(idx);
            shells.insert(0, head);
        }
    }
    shells
}

#[cfg(windows)]
fn scan_windows() -> Vec<String> {
    use std::path::Path;

    let mut candidates: Vec<String> = Vec::new();

    // 1) 已知绝对路径 —— Windows 没有 /etc/shells 等价物，硬编码常见安装位置 + 验存在。
    //    SystemRoot 通常是 C:\Windows，但企业镜像可能改过，所以读环境变量而不写死。
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let known: &[String] = &[
        format!("{system_root}\\System32\\cmd.exe"),
        format!("{system_root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
        format!("{system_root}\\System32\\wsl.exe"),
        "C:\\Program Files\\PowerShell\\7\\pwsh.exe".to_string(),
        "C:\\Program Files\\Git\\bin\\bash.exe".to_string(),
        "C:\\Program Files\\Git\\usr\\bin\\bash.exe".to_string(),
    ];
    for c in known {
        if is_shell_candidate(Path::new(c)) {
            candidates.push(c.clone());
        }
    }

    // 2) PATH 扫已知名字 —— 捞 winget/scoop 装的 pwsh / nu / fish 等。
    const KNOWN_WIN: &[&str] = &[
        "pwsh.exe",
        "bash.exe",
        "nu.exe",
        "fish.exe",
        "elvish.exe",
        "xonsh.exe",
    ];
    if let Ok(path_env) = std::env::var("PATH") {
        for dir in path_env.split(';').filter(|d| !d.is_empty()) {
            for name in KNOWN_WIN {
                let candidate = format!("{dir}\\{name}");
                if is_shell_candidate(Path::new(&candidate)) {
                    candidates.push(candidate);
                }
            }
        }
    }

    // canonical 去重 + 排序。Windows junction point 少，主要是吃掉 PATH 里
    // 重复目录导致的同一路径多次 push。
    let mut shells = dedup_by_canonical(candidates);
    shells.sort();
    shells
}

fn default_shell() -> String {
    // SHELL 仅 Unix 上可信：Windows 下 MSYS/Git Bash 常把 SHELL 设为
    // /usr/bin/bash 这种 Unix 路径，portable_pty 拿去 spawn 会直接失败。
    // Windows 走 available_shells() 的扫描结果（System32 / Program Files / PATH）。
    // 即便在 Unix，也得校验 SHELL 真有效（trim + is_shell_candidate）—— 空串、
    // 卸载残留的旧路径、user 手改坏的值都得过滤，避免拿垃圾路径去 spawn。
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(s) = std::env::var("SHELL") {
            let trimmed = s.trim();
            if !trimmed.is_empty() && is_shell_candidate(std::path::Path::new(trimmed)) {
                return trimmed.to_string();
            }
        }
        available_shells()
            .into_iter()
            .next()
            .unwrap_or_else(|| "/bin/sh".to_string())
    }
    #[cfg(target_os = "windows")]
    {
        // Windows 没有 $SHELL 等价物 —— available_shells() 是字典序排好的，
        // 直接拿 first 会让 `C:\Program Files\Git\bin\bash.exe` 这种偏门项目
        // 在 `C:\Windows\System32\cmd.exe` 之前。显式按偏好（cmd > pwsh >
        // powershell）挑，挑不到再退到字典序首位。
        let shells = available_shells();
        const PREF_SUFFIXES: &[&str] = &["\\cmd.exe", "\\pwsh.exe", "\\powershell.exe"];
        for suf in PREF_SUFFIXES {
            if let Some(s) = shells.iter().find(|s| s.to_lowercase().ends_with(suf)) {
                return s.clone();
            }
        }
        shells
            .into_iter()
            .next()
            .unwrap_or_else(|| "cmd.exe".to_string())
    }
}

pub fn spawn(
    session_id: String,
    cols: u16,
    rows: u16,
    sink: PtySink,
    shell_override: Option<String>,
) -> AppResult<(String, PtyHandle)> {
    let shell = shell_override
        .filter(|s| !s.is_empty())
        .unwrap_or_else(default_shell);
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("RSSH_APP", "1");
    if !cfg!(target_os = "windows") {
        cmd.arg("-l");
    }
    spawn_builder(session_id, cols, rows, sink, cmd, shell, true)
}

/// Start a specific local program under a PTY. Used by dynamic connectors such
/// as `docker exec` / `kubectl exec`: the frontend still sees the same PTY
/// transport, but the first process is the connector command instead of the
/// user's login shell.
pub fn spawn_command(
    session_id: String,
    cols: u16,
    rows: u16,
    sink: PtySink,
    program: PathBuf,
    search_path: OsString,
    args: Vec<String>,
) -> AppResult<(String, PtyHandle)> {
    let mut cmd = CommandBuilder::new(&program);
    cmd.env("PATH", search_path);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("RSSH_APP", "1");
    for arg in args {
        cmd.arg(arg);
    }
    let program_label = program.to_string_lossy().into_owned();
    spawn_builder(session_id, cols, rows, sink, cmd, program_label, false)
}

fn spawn_builder(
    session_id: String,
    cols: u16,
    rows: u16,
    sink: PtySink,
    cmd: CommandBuilder,
    shell_path: String,
    track_shell_foreground: bool,
) -> AppResult<(String, PtyHandle)> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| AppError::pty("pty_op_failed", serde_json::json!({ "err": e.to_string() })))?;

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| AppError::pty("pty_op_failed", serde_json::json!({ "err": e.to_string() })))?;
    drop(pair.slave);

    #[cfg(unix)]
    let foreground_process = track_shell_foreground
        .then(|| child.process_id())
        .flatten()
        .and_then(|pid| libc::pid_t::try_from(pid).ok())
        .map(ForegroundProcessState::new)
        .map(Mutex::new)
        .map(Arc::new);
    #[cfg(not(unix))]
    let _ = track_shell_foreground;

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| AppError::pty("pty_op_failed", serde_json::json!({ "err": e.to_string() })))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| AppError::pty("pty_op_failed", serde_json::json!({ "err": e.to_string() })))?;

    let master = Arc::new(Mutex::new(pair.master));
    let pty_id: Arc<str> = Arc::from(session_id.as_str());
    let handle = PtyHandle {
        writer: Arc::new(Mutex::new(writer)),
        master: Arc::clone(&master),
        shell_path: Arc::from(shell_path.as_str()),
        #[cfg(unix)]
        session_id: Arc::clone(&pty_id),
        #[cfg(unix)]
        sink: Arc::clone(&sink),
        #[cfg(unix)]
        foreground_process: foreground_process.clone(),
        _reaper: Arc::new(ChildReaper {
            child: Mutex::new(Some(child)),
        }),
    };

    // 读取线程：PTY stdout → Tauri 事件
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut reader = reader;
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    #[cfg(unix)]
                    let returned_to_shell =
                        observe_foreground_process(&master, foreground_process.as_ref());
                    sink(&pty_id, PtyOut::Data(buf[..n].to_vec()));
                    #[cfg(unix)]
                    if returned_to_shell {
                        // Queue the local reset after the bytes that exposed
                        // this process-group transition. The pending flag above
                        // still guards against older PTY bytes arriving later.
                        sink(&pty_id, PtyOut::ShellForeground);
                    }
                }
            }
        }
        sink(&pty_id, PtyOut::Close);
    });

    Ok((session_id, handle))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn foreground_process_group(handle: &PtyHandle) -> Option<libc::pid_t> {
        locked(&handle.master)
            .ok()
            .and_then(|master| master.process_group_leader())
    }

    fn wait_for_process_group(
        handle: &PtyHandle,
        predicate: impl Fn(libc::pid_t) -> bool,
    ) -> libc::pid_t {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(pgrp) = foreground_process_group(handle) {
                if predicate(pgrp) {
                    return pgrp;
                }
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for foreground process group"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn foreground_state_reports_only_child_to_shell_transitions() {
        let mut state = ForegroundProcessState::new(100);

        assert!(!state.observe(100));
        assert!(!state.observe(200));
        assert!(!state.observe(201));
        assert!(state.observe(100));
        assert!(!state.observe(100));

        assert!(!state.observe(300));
        assert!(state.observe(100));
    }

    #[test]
    fn shell_return_stays_pending_until_the_stale_mouse_report_is_discarded() {
        let mut state = ForegroundProcessState::new(100);

        assert!(!state.observe(200));
        assert!(state.observe(100));
        assert!(!state.take_stale_mouse_report(b"x"));
        assert!(state.take_stale_mouse_report(b"\x1b[<35;104;61M"));
        assert!(!state.take_stale_mouse_report(b"\x1b[<35;104;61M"));

        assert!(!state.observe(300));
        assert!(!state.take_stale_mouse_report(b"\x1b[<35;104;61M"));
    }

    #[test]
    fn mouse_report_detection_does_not_swallow_other_terminal_input() {
        assert!(is_xterm_mouse_report(b"\x1b[<35;104;61M"));
        assert!(is_xterm_mouse_report(b"\x1b[<0;1;1m"));
        assert!(is_xterm_mouse_report(b"\x1b[M !!"));

        assert!(!is_xterm_mouse_report(b"\x1b[<35;104;61"));
        assert!(!is_xterm_mouse_report(b"\x1b[<35;104M"));
        assert!(!is_xterm_mouse_report(b"printf '\x1b[<35;104;61M'"));
        assert!(!is_xterm_mouse_report(b"\x1b[A"));
    }

    #[test]
    fn foreground_process_group_returns_to_shell_after_ctrl_c() {
        let sink: PtySink = Arc::new(|_, _| {});
        let (_, handle) = spawn(
            "pgrp-test".to_string(),
            80,
            24,
            sink,
            Some("/bin/sh".to_string()),
        )
        .expect("spawn shell");

        let shell_pgrp = wait_for_process_group(&handle, |_| true);
        handle.write(b"sleep 30\r").expect("start foreground child");
        let child_pgrp = wait_for_process_group(&handle, |pgrp| pgrp != shell_pgrp);
        assert_ne!(child_pgrp, shell_pgrp);

        handle.write(&[0x03]).expect("send Ctrl-C");
        let returned_pgrp = wait_for_process_group(&handle, |pgrp| pgrp == shell_pgrp);
        assert_eq!(returned_pgrp, shell_pgrp);
    }
}
