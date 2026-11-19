//! A command observes bytes from the existing transport reader. Each transport
//! attempt owns a fresh tap; a capture never owns the PTY child or SSH channel.

use std::collections::VecDeque;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use regex::bytes::Regex as ByteRegex;
use regex::Regex;
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::error::{locked, AppError, AppResult};

const HEAD_LIMIT: usize = 512 * 1024;
const TAIL_LIMIT: usize = 4 * 1024;

#[derive(Clone, Default)]
pub struct OutputTap(Arc<Mutex<TapState>>);

#[derive(Default)]
struct TapState {
    closed: bool,
    active: Weak<ActiveCapture>,
}

impl OutputTap {
    pub fn is_closed(&self) -> bool {
        self.0.lock().expect("terminal tap lock").closed
    }

    /// Only one operation can claim a terminal at a time, including operations
    /// initiated by different actors. An empty sentinel selects manual completion.
    pub fn start(&self, sentinel: &str) -> AppResult<CommandCapture> {
        let mut tap = locked(&self.0)?;
        if tap.closed {
            return Err(AppError::other(
                "session_channel_closed",
                serde_json::json!({}),
            ));
        }
        if tap.active.upgrade().is_some() {
            return Err(AppError::other("agent_session_busy", serde_json::json!({})));
        }
        let (tx, rx) = oneshot::channel();
        let active = Arc::new(ActiveCapture(Mutex::new(Some(Collecting {
            output: BoundedOutput::default(),
            marker: if sentinel.is_empty() {
                None
            } else {
                Some(Marker::new(sentinel))
            },
            tx,
        }))));
        tap.active = Arc::downgrade(&active);
        Ok(CommandCapture { active, rx })
    }

    fn active(&self) -> Option<Arc<ActiveCapture>> {
        self.0.lock().expect("terminal tap lock").active.upgrade()
    }

    fn publish_chunk<B: AsRef<[u8]>>(&self, bytes: B, display: impl FnOnce(B)) {
        // Submit/Stop can arrive as soon as display enqueues the event.
        // Record its bytes first, but deliver a shell exit after the event.
        let exited = self.active().and_then(|active| {
            let code = active.feed(bytes.as_ref())?;
            Some((active, code))
        });
        display(bytes);
        if let Some((active, code)) = exited {
            active.finish(Completion::Exit(code));
        }
    }

    pub fn publish_ref(&self, bytes: &[u8], display: impl FnOnce()) {
        self.publish_chunk(bytes, |_| display());
    }

    pub fn publish(&self, bytes: Vec<u8>, display: impl FnOnce(Vec<u8>)) {
        self.publish_chunk(bytes, display);
    }

    #[cfg(test)]
    fn feed(&self, bytes: &[u8]) {
        self.publish_ref(bytes, || {});
    }

    /// Sticky: a delayed approval cannot register on a disconnected attempt.
    pub fn close(&self) {
        let mut tap = self.0.lock().expect("terminal tap lock");
        tap.closed = true;
        if let Some(active) = tap.active.upgrade() {
            active.finish(Completion::Interrupted);
        }
    }
}

/// Construct the bytes between the paste markers. The transport appends the
/// submit key (or its configured Telnet line ending) after the closing marker.
pub fn command_body(command: &str, bracketed: bool) -> String {
    if bracketed {
        format!("\x1b[200~{command}\x1b[201~")
    } else {
        command.to_owned()
    }
}

#[derive(Debug)]
pub struct CapturedOutput {
    pub output: String,
    pub exit_code: i32,
    pub timed_out: bool,
    pub early_terminated: bool,
}

pub struct CommandCapture {
    active: Arc<ActiveCapture>,
    rx: oneshot::Receiver<Completed>,
}

impl CommandCapture {
    /// Cancellation-safe in select!: the deadline belongs to the operation, not
    /// to an individual wait. Timing out detaches collection without writing ETX.
    pub async fn wait(&mut self, deadline: Instant) -> CapturedOutput {
        let completed = match tokio::time::timeout_at(deadline, &mut self.rx).await {
            Ok(completed) => completed,
            Err(_) => {
                self.active.finish(Completion::Timeout);
                (&mut self.rx).await
            }
        };
        completed
            .expect("active capture owns the completion sender")
            .into_result()
    }

    /// Only the caller that wins this race should send Ctrl+C to a shell.
    /// Dropping the capture releases the exclusive slot after that write.
    pub fn interrupt(&mut self) -> Option<CapturedOutput> {
        self.finish(Completion::Interrupted)
    }

    /// Raw devices have no exit-code protocol. The user can accept partial
    /// output as a normal result without sending an interrupt to the peer.
    pub fn submit(&mut self) -> Option<CapturedOutput> {
        self.finish(Completion::Manual)
    }

    fn finish(&mut self, reason: Completion) -> Option<CapturedOutput> {
        self.active.finish(reason).then(|| {
            self.rx
                .try_recv()
                .expect("completion was delivered synchronously")
                .into_result()
        })
    }
}

struct ActiveCapture(Mutex<Option<Collecting>>);

impl ActiveCapture {
    fn feed(&self, bytes: &[u8]) -> Option<i32> {
        let mut slot = self.0.lock().expect("active capture lock");
        let run = slot.as_mut()?;
        if let Some(marker) = &mut run.marker {
            marker.feed(&mut run.output, bytes)
        } else {
            run.output.append(bytes);
            None
        }
    }

    fn finish(&self, reason: Completion) -> bool {
        let mut slot = self.0.lock().expect("active capture lock");
        if matches!(reason, Completion::Manual)
            && slot.as_ref().is_some_and(|run| run.marker.is_some())
        {
            return false;
        }
        if let Some(run) = slot.take() {
            run.complete(reason);
            true
        } else {
            false
        }
    }
}

enum Completion {
    Exit(i32),
    Timeout,
    Interrupted,
    Manual,
}

struct Collecting {
    output: BoundedOutput,
    marker: Option<Marker>,
    tx: oneshot::Sender<Completed>,
}

impl Collecting {
    fn complete(mut self, reason: Completion) {
        if let Some(marker) = &self.marker {
            self.output.append(&marker.pending);
        }
        let _ = self.tx.send(Completed {
            bytes: self.output,
            reason,
        });
    }
}

struct Completed {
    bytes: BoundedOutput,
    reason: Completion,
}

impl Completed {
    fn into_result(self) -> CapturedOutput {
        // Decode and strip terminal controls on the consumer, never the reader.
        static CONTROLS: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\r").unwrap()
        });
        let bytes = self.bytes.into_vec();
        let decoded = String::from_utf8_lossy(&bytes);
        let text = CONTROLS.replace_all(&decoded, "");
        CapturedOutput {
            output: text.trim_end().to_owned(),
            exit_code: match self.reason {
                Completion::Exit(code) => code,
                Completion::Manual => 0,
                _ => -1,
            },
            timed_out: matches!(self.reason, Completion::Timeout),
            early_terminated: matches!(self.reason, Completion::Interrupted),
        }
    }
}

/// Keep a prefix for the actual result and a disjoint suffix for the most
/// recent output. Each input byte is copied at most once into these windows.
#[derive(Default)]
struct BoundedOutput {
    head: Vec<u8>,
    tail: VecDeque<u8>,
}

impl BoundedOutput {
    fn append(&mut self, bytes: &[u8]) {
        let (head, tail) = bytes.split_at((HEAD_LIMIT - self.head.len()).min(bytes.len()));
        self.head.extend_from_slice(head);
        let tail = &tail[tail.len().saturating_sub(TAIL_LIMIT)..];
        self.tail
            .drain(..(self.tail.len() + tail.len()).saturating_sub(TAIL_LIMIT));
        self.tail.extend(tail);
    }

    fn into_vec(mut self) -> Vec<u8> {
        self.head.extend(self.tail);
        self.head
    }
}

/// Search only the incoming chunk and enough overlap for a marker and signed
/// i32. Wait for a non-digit (usually CR, LF, or a terminal control byte)
/// so a split "130" cannot be reported as "1".
struct Marker {
    pattern: ByteRegex,
    overlap: usize,
    pending: Vec<u8>,
    skipping_echo: bool,
    matched_exit: bool,
}

impl Marker {
    fn new(sentinel: &str) -> Self {
        // At the terminal's right margin readline can emit a space followed by
        // a carriage return before drawing the next input character. Match
        // that redraw only in the echoed template; actual output stays intact.
        let echoed = sentinel
            .chars()
            .chain(std::iter::once(':'))
            .map(|ch| format!(r"{}(?: \r)?", regex::escape(&ch.to_string())))
            .collect::<String>();
        let pattern = format!(
            r"{}:(-?[0-9]{{1,10}})(?-u:[^0-9])|{echoed}[$%]",
            regex::escape(sentinel)
        );
        Self {
            pattern: ByteRegex::new(&pattern).expect("escaped internal marker"),
            overlap: ((sentinel.len() + 1) * 3).max(sentinel.len() + 12),
            pending: Vec::new(),
            skipping_echo: false,
            matched_exit: false,
        }
    }

    fn feed(&mut self, output: &mut BoundedOutput, bytes: &[u8]) -> Option<i32> {
        // Ignore later frames while the display sink enqueues the exit frame.
        // The capture itself stays available for a timeout or explicit stop.
        if self.matched_exit {
            return None;
        }
        // A transport (in particular a native adapter) can deliver a frame
        // larger than the capture limit. Bound scratch space independently.
        bytes
            .chunks(8 * 1024)
            .find_map(|chunk| self.scan(output, chunk))
    }

    fn scan(&mut self, output: &mut BoundedOutput, bytes: &[u8]) -> Option<i32> {
        self.pending.extend_from_slice(bytes);
        let mut from = 0;
        loop {
            if self.skipping_echo {
                // The literal marker in the input template (:$? / :$(...) /
                // :%errorlevel%) identifies the entire pasted command, even
                // when a login banner or repeated prompt/redraw preceded it.
                let Some(newline) = self.pending[from..].iter().position(|byte| *byte == b'\n')
                else {
                    self.pending.clear();
                    return None;
                };
                from += newline + 1;
                self.skipping_echo = false;
            }

            let Some(hit) = self.pattern.captures_at(&self.pending, from) else {
                break;
            };
            let matched = hit.get(0).expect("matching marker");
            if let Some(digits) = hit.get(1) {
                if let Ok(code) = std::str::from_utf8(digits.as_bytes())
                    .expect("ASCII exit code")
                    .parse()
                {
                    output.append(&self.pending[from..matched.start()]);
                    self.pending.clear();
                    self.matched_exit = true;
                    return Some(code);
                }
                // Preserve malformed output and keep looking for the real marker.
                output.append(&self.pending[from..matched.end()]);
            } else {
                // Reclaim the whole output budget, even if the echoed input
                // itself was larger than the head window. With echo disabled
                // this branch never runs and the first output line is retained.
                *output = BoundedOutput::default();
                self.skipping_echo = true;
            }
            from = matched.end();
        }
        let keep_from = self.pending.len().saturating_sub(self.overlap).max(from);
        output.append(&self.pending[from..keep_from]);
        self.pending.drain(..keep_from);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const MARKER: &str = "__rssh_done_a.b*c";

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(3)
    }

    #[test]
    fn paste_wraps_only_the_command_body() {
        assert_eq!(command_body("echo 中文", false), "echo 中文");
        assert_eq!(command_body("a\nb", true), "\x1b[200~a\nb\x1b[201~");
    }

    #[tokio::test]
    async fn display_precedes_completion_and_new_subscriptions() {
        let tap = OutputTap::default();
        let bytes = format!("visible\r\n{MARKER}:0\r\n").into_bytes();
        let mut subscribed_during_display = None;
        tap.publish(bytes.clone(), |visible| {
            assert_eq!(visible, bytes);
            subscribed_during_display = Some(tap.start(MARKER).unwrap());
        });
        let mut capture = subscribed_during_display.unwrap();
        tap.publish_ref(&bytes, || {
            tap.feed(b"late frame");
            assert!(matches!(
                capture.rx.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
        });
        assert_eq!(capture.wait(deadline()).await.output, "visible");
    }

    #[test]
    fn finishing_from_display_includes_the_visible_chunk() {
        let tap = OutputTap::default();
        let mut raw = tap.start("").unwrap();
        tap.publish(b"device online\r\n".to_vec(), |_| {
            assert_eq!(raw.submit().unwrap().output, "device online");
        });
        drop(raw);

        let mut shell = tap.start(MARKER).unwrap();
        tap.publish_ref(b"still running\r\n", || {
            assert_eq!(shell.interrupt().unwrap().output, "still running");
        });
    }

    #[tokio::test]
    async fn a_blocked_display_cannot_extend_the_capture_deadline() {
        let tap = OutputTap::default();
        let mut capture = tap.start(MARKER).unwrap();
        let (entered, displaying) = oneshot::channel();
        let (resume, blocked) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            tap.publish_ref(format!("visible{MARKER}:0\r\n").as_bytes(), || {
                entered.send(()).unwrap();
                let _ = blocked.recv_timeout(Duration::from_secs(5));
            });
        });
        displaying.await.unwrap();
        let result = tokio::time::timeout_at(deadline(), capture.wait(Instant::now())).await;
        let _ = resume.send(());
        reader.join().unwrap();
        let result = result.expect("the capture must time out while display is blocked");
        assert!(result.timed_out);
        assert_eq!(result.output, "visible");
    }

    #[tokio::test]
    async fn matches_every_chunk_boundary_and_preserves_unterminated_output() {
        let stream = format!(
            "$ shell ; echo \"{MARKER}:$?\"\r\n\x1b]0;title\x07\x1b[32m  中文🙂\x1b[0m{MARKER}:-130\r\nprompt"
        );
        for boundary in 0..=stream.len() {
            let tap = OutputTap::default();
            let mut capture = tap.start(MARKER).unwrap();
            let (a, b) = stream.as_bytes().split_at(boundary);
            tap.feed(a);
            tap.feed(b);
            let result = capture.wait(deadline()).await;
            assert_eq!(result.output, "  中文🙂", "split at {boundary}");
            assert_eq!(result.exit_code, -130, "split at {boundary}");
            assert!(!result.early_terminated && !result.timed_out);
        }
    }

    #[tokio::test]
    async fn echo_disabled_preserves_first_output_line() {
        let tap = OutputTap::default();
        let mut capture = tap.start(MARKER).unwrap();
        tap.feed(
            format!("\x1b]133;C\x1b\\first\r\n\x1b]0;title\x07second{MARKER}:42\x1b[0m\r\n")
                .as_bytes(),
        );
        let output = capture.wait(deadline()).await;
        assert_eq!(output.output, "first\nsecond");
        assert_eq!(output.exit_code, 42);
    }

    #[tokio::test]
    async fn discards_long_multiline_input_and_repeated_shell_redraws() {
        let tap = OutputTap::default();
        let mut capture = tap.start(MARKER).unwrap();
        tap.feed(b"login banner\r\nprompt> ");
        tap.feed(&vec![b'x'; HEAD_LIMIT + TAIL_LIMIT]);
        tap.feed(format!("\r\ncontinued; echo \"{MARKER}:$?\"\r\n").as_bytes());
        tap.feed(
            format!("prompt> command; echo {MARKER}:$?\r\nactual\r\n{MARKER}:0\r\n").as_bytes(),
        );
        assert_eq!(capture.wait(deadline()).await.output, "actual");
    }

    #[tokio::test]
    async fn readline_can_wrap_anywhere_in_the_echoed_marker() {
        let tap = OutputTap::default();
        let template = format!("{MARKER}:");
        for wrap in 1..=template.len() {
            let mut capture = tap.start(MARKER).unwrap();
            let (before, after) = template.split_at(wrap);
            tap.feed(format!("prompt $ echo next ; echo \"{before} ").as_bytes());
            tap.feed(format!("\r{after}$?\"\r\nnext\r\n{MARKER}:0\r\n").as_bytes());
            assert_eq!(
                capture.wait(deadline()).await.output,
                "next",
                "wrap at {wrap}"
            );
        }
    }

    #[tokio::test]
    async fn keeps_disjoint_head_and_tail_across_a_large_frame() {
        let tap = OutputTap::default();
        let mut capture = tap.start(MARKER).unwrap();
        let echo = format!("command {MARKER}:$?\r\n");
        let mut frame = echo.as_bytes().to_vec();
        frame.extend(vec![b'a'; HEAD_LIMIT]);
        frame.extend(vec![b'b'; TAIL_LIMIT + 2]);
        frame.extend(vec![b'z'; TAIL_LIMIT]);
        frame.extend_from_slice(format!("{MARKER}:42\n").as_bytes());
        frame.extend(vec![b'x'; TAIL_LIMIT * 3]); // late bytes never hide the marker
        tap.feed(&frame);
        let result = capture.wait(deadline()).await;
        assert_eq!(result.exit_code, 42);
        assert_eq!(
            result.output,
            "a".repeat(HEAD_LIMIT) + &"z".repeat(TAIL_LIMIT)
        );
    }

    #[tokio::test]
    async fn timeout_returns_partial_output_and_does_not_interrupt() {
        let tap = OutputTap::default();
        let mut capture = tap.start(MARKER).unwrap();
        tap.feed(format!("cmd {MARKER}:$?\r\nrunning").as_bytes());
        let result = capture.wait(Instant::now()).await;
        assert_eq!(result.output, "running");
        assert_eq!(result.exit_code, -1);
        assert!(result.timed_out);
        assert!(!result.early_terminated);
        assert!(capture.interrupt().is_none());
    }

    #[tokio::test]
    async fn natural_completion_wins_over_an_interrupt() {
        let tap = OutputTap::default();
        let mut capture = tap.start(MARKER).unwrap();
        assert!(capture.submit().is_none()); // shell completion requires an exit code
        tap.feed(format!("echo {MARKER}:$?\nok\n{MARKER}:0\n").as_bytes());
        assert!(capture.interrupt().is_none());
        assert_eq!(capture.wait(deadline()).await.output, "ok");
    }

    #[tokio::test]
    async fn interrupt_keeps_its_slot_until_the_transport_write_finishes() {
        let tap = OutputTap::default();
        let mut capture = tap.start(MARKER).unwrap();
        tap.feed(format!("cmd {MARKER}:$?\r\npart").as_bytes());
        let result = capture.interrupt().unwrap();
        assert_eq!(result.output, "part");
        assert!(result.early_terminated && !result.timed_out);
        assert_eq!(result.exit_code, -1);
        assert!(tap.start(MARKER).is_err());
        assert!(capture.interrupt().is_none());
        drop(capture);
        assert!(tap.start(MARKER).is_ok());
    }

    #[tokio::test]
    async fn closed_attempt_and_new_attempt_are_isolated() {
        let old = OutputTap::default();
        let new = OutputTap::default();
        let mut first = old.start(MARKER).unwrap();
        let mut second = new.start(MARKER).unwrap();
        old.feed(format!("cmd {MARKER}:$?\r\nold").as_bytes());
        old.close();
        assert!(old.is_closed());
        assert!(old.start(MARKER).is_err());
        old.feed(format!("{MARKER}:1\n").as_bytes());
        new.feed(format!("cmd {MARKER}:$?\r\nnew{MARKER}:9\n").as_bytes());
        let old_result = first.wait(deadline()).await;
        assert!(old_result.early_terminated);
        assert_eq!(old_result.output, "old");
        assert!(!new.is_closed());
        let new_result = second.wait(deadline()).await;
        assert_eq!(new_result.output, "new");
        assert_eq!(new_result.exit_code, 9);
    }

    #[test]
    fn raw_device_keeps_first_line_and_completes_only_on_request() {
        let tap = OutputTap::default();
        let mut capture = tap.start("").unwrap();
        tap.feed(format!("first\r\n{MARKER}:9\r\n").as_bytes());
        let result = capture.submit().unwrap();
        assert!(!result.timed_out && !result.early_terminated);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.output, format!("first\n{MARKER}:9"));
    }
}
