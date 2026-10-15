import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

// Compile the actual patched queue against a fake native window boundary. The
// worker threads, operation ordering and Drop behavior remain production code.
test('Tao windows keep independent lifetimes during closure and native creation', (context) => {
  const directory = mkdtempSync(path.join(tmpdir(), 'rssh-tao-window-queue-'));
  try {
    const source = process.env.RSSH_TAO_QUEUE_SOURCE ?? fileURLToPath(new URL('../../target/ohos-sources/tao/src/platform_impl/ohos/window_queue.rs', import.meta.url));
    const harness = path.join(directory, 'window-queue-tests.rs');
    const binary = path.join(directory, process.platform === 'win32' ? 'window-queue-tests.exe' : 'window-queue-tests');
    writeFileSync(harness, `
extern crate self as openharmony_ability;
extern crate self as openharmony_ability_plugin_window;
extern crate self as futures_executor;
extern crate self as log;

use std::{future::Future, pin::pin, sync::{mpsc, Arc, Condvar, Mutex}, task::{Context, Poll, Wake, Waker}, time::Duration};
#[macro_export]
macro_rules! error { ($($args:tt)*) => { eprintln!($($args)*); }; }

#[derive(Clone, Debug, Default)]
pub struct ManagedWindowRequest {
    pub window_id: i64, pub x: i32, pub y: i32, pub width: i32, pub height: i32,
    pub visible: bool, pub maximized: bool, pub title: String, pub decorations: bool, pub resizable: bool,
}
#[derive(Clone, Debug)]
pub struct ManagedWindowCommand {
    pub window_id: i64, pub operation: String, pub value: Option<bool>, pub text: Option<String>,
    pub x: Option<i32>, pub y: Option<i32>,
}
#[derive(Default)]
pub struct WindowEvent {
    pub window_id: i64, pub kind: String, pub x: i32, pub y: i32, pub width: i32, pub height: i32,
    pub visible: bool, pub maximized: bool, pub focused: bool, pub minimized: bool, pub scale: f64,
}
pub fn observe_window(_: i64, _: impl Fn(WindowEvent) + Send + Sync + 'static) {}
#[derive(Clone, Debug, PartialEq)]
enum NativeCall { Attach(i64), Update(i64, String) }
type Gate = Arc<(Mutex<bool>, Condvar)>;
#[derive(Clone)]
pub struct OpenHarmonyApp { calls: mpsc::Sender<NativeCall>, gate: Option<(i64, Gate)> }
#[derive(Clone)]
pub struct OpenHarmonyWaker;
impl OpenHarmonyWaker { pub fn wake(&self) {} }
impl OpenHarmonyApp { pub fn create_waker(&self) -> OpenHarmonyWaker { OpenHarmonyWaker } }
pub trait WindowExt { fn window(&self) -> Result<NativeClient, String>; }
impl WindowExt for OpenHarmonyApp {
    fn window(&self) -> Result<NativeClient, String> { Ok(NativeClient(self.clone())) }
}
pub struct NativeClient(OpenHarmonyApp);
impl NativeClient {
    pub async fn attach_managed_window(&self, request: ManagedWindowRequest) -> Result<(), String> {
        self.0.calls.send(NativeCall::Attach(request.window_id)).unwrap();
        if let Some((id, gate)) = &self.0.gate {
            if *id == request.window_id {
                let (lock, condition) = &**gate;
                let mut released = lock.lock().unwrap();
                while !*released { released = condition.wait(released).unwrap(); }
            }
        }
        Ok(())
    }
    pub async fn update_managed_window(&self, command: ManagedWindowCommand) -> Result<(), String> {
        self.0.calls.send(NativeCall::Update(command.window_id, command.operation)).unwrap();
        Ok(())
    }
}
struct ThreadWake;
impl Wake for ThreadWake { fn wake(self: Arc<Self>) {} }
pub fn block_on<T>(future: impl Future<Output = T>) -> T {
    let waker = Waker::from(Arc::new(ThreadWake));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop { if let Poll::Ready(value) = future.as_mut().poll(&mut context) { return value; } }
}

#[path = ${JSON.stringify(source)}]
mod window_queue;
use window_queue::{WindowQueue, WindowState};

fn start(app: &OpenHarmonyApp, id: i64, events: &mpsc::Sender<WindowEvent>) -> WindowQueue {
    let state = Arc::new(Mutex::new(WindowState::new(ManagedWindowRequest { window_id: id, ..Default::default() }, 1.0)));
    WindowQueue::start(app.clone(), state, events.clone()).unwrap()
}
fn command(id: i64, operation: &str) -> ManagedWindowCommand {
    ManagedWindowCommand { window_id: id, operation: operation.into(), value: None, text: None, x: None, y: None }
}
fn receive<T>(receiver: &mpsc::Receiver<T>) -> T { receiver.recv_timeout(Duration::from_secs(3)).unwrap() }
fn fixture(gate: Option<(i64, Gate)>) -> (OpenHarmonyApp, mpsc::Receiver<NativeCall>, mpsc::Sender<WindowEvent>, mpsc::Receiver<WindowEvent>) {
    let (calls, received) = mpsc::channel();
    let (events, event_received) = mpsc::channel();
    (OpenHarmonyApp { calls, gate }, received, events, event_received)
}
fn release(gate: &Gate) { let (lock, condition) = &**gate; *lock.lock().unwrap() = true; condition.notify_all(); }
fn closed(events: &mpsc::Receiver<WindowEvent>, id: i64) {
    let event = receive(events); assert_eq!(event.window_id, id); assert_eq!(event.kind, "destroyed");
}

#[test]
fn closing_first_window_preserves_existing_and_new_windows() {
    let (app, calls, events, received) = fixture(None);
    let first = start(&app, 0, &events);
    assert_eq!(receive(&calls), NativeCall::Attach(0));
    let second = start(&app, 1, &events);
    assert_eq!(receive(&calls), NativeCall::Attach(1));
    drop(first);
    assert_eq!(receive(&calls), NativeCall::Update(0, "destroy".into()));
    closed(&received, 0);
    second.send(command(1, "resize"));
    assert_eq!(receive(&calls), NativeCall::Update(1, "resize".into()));
    let third = start(&app, 2, &events);
    assert_eq!(receive(&calls), NativeCall::Attach(2));
    drop(second);
    assert_eq!(receive(&calls), NativeCall::Update(1, "destroy".into()));
    closed(&received, 1);
    third.send(command(2, "focus"));
    assert_eq!(receive(&calls), NativeCall::Update(2, "focus".into()));
    drop(third);
    assert_eq!(receive(&calls), NativeCall::Update(2, "destroy".into()));
    closed(&received, 2);
}

#[test]
fn first_window_closure_does_not_cancel_sibling_creation() {
    let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (app, calls, events, received) = fixture(Some((1, gate.clone())));
    let first = start(&app, 0, &events);
    assert_eq!(receive(&calls), NativeCall::Attach(0));
    let second = start(&app, 1, &events);
    assert_eq!(receive(&calls), NativeCall::Attach(1));
    drop(first);
    assert_eq!(receive(&calls), NativeCall::Update(0, "destroy".into()));
    closed(&received, 0);
    release(&gate);
    second.send(command(1, "move"));
    assert_eq!(receive(&calls), NativeCall::Update(1, "move".into()));
    drop(second);
    assert_eq!(receive(&calls), NativeCall::Update(1, "destroy".into()));
    closed(&received, 1);
}

#[test]
fn closing_a_window_during_its_creation_orders_its_own_cleanup() {
    let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (app, calls, events, received) = fixture(Some((5, gate.clone())));
    let window = start(&app, 5, &events);
    assert_eq!(receive(&calls), NativeCall::Attach(5));
    drop(window);
    release(&gate);
    assert_eq!(receive(&calls), NativeCall::Update(5, "destroy".into()));
    closed(&received, 5);
    assert!(received.try_recv().is_err());
}
`);
    execFileSync('rustc', ['--edition=2021', '--test', harness, '-o', binary], { stdio: 'pipe' });
    const result = spawnSync(binary, ['--test-threads=1'], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stdout + result.stderr);
    context.diagnostic(result.stdout.split('\n').find((line) => line.startsWith('test result:')));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('Tao initializes once from an ArkUI stage or a legacy render surface', () => {
  const directory = mkdtempSync(path.join(tmpdir(), 'rssh-tao-window-init-'));
  try {
    const queue = process.env.RSSH_TAO_QUEUE_SOURCE ?? fileURLToPath(new URL('../../target/ohos-sources/tao/src/platform_impl/ohos/window_queue.rs', import.meta.url));
    const source = readFileSync(path.join(path.dirname(queue), 'mod.rs'), 'utf8');
    const arms = source.match(/        MainEvent::WindowCreate => \{[\s\S]*?(?=        MainEvent::SurfaceDestroy)/)?.[0];
    assert.ok(arms, 'WindowStage creation must be able to start the event loop without an XComponent');
    const harness = path.join(directory, 'init.rs');
    const binary = path.join(directory, 'init-tests');
    writeFileSync(harness, `
use std::{cell::{Cell, RefCell}, rc::Rc};
enum MainEvent { WindowCreate, SurfaceCreate }
enum StartCause { Init }
struct WindowId(u64);
mod window { pub struct WindowId(pub super::WindowId); }
mod event {
    pub enum WindowEvent { Resumed }
    pub enum Event { NewEvents(super::StartCause), WindowEvent { window_id: super::window::WindowId, event: WindowEvent } }
}
struct EventLoop { initialized: Cell<bool>, event_handler: RefCell<Option<Box<dyn FnMut(event::Event)>>> }
impl EventLoop { fn deliver(&self, event: MainEvent) { match event { ${arms} } } }
fn check(events: &[MainEvent]) {
    let initialized = Rc::new(Cell::new(0));
    let counter = initialized.clone();
    let event_loop = EventLoop { initialized: Cell::new(false), event_handler: RefCell::new(Some(Box::new(move |event| {
        if let event::Event::NewEvents(StartCause::Init) = event { counter.set(counter.get() + 1); }
    }))) };
    for event in events { event_loop.deliver(match event { MainEvent::WindowCreate => MainEvent::WindowCreate, MainEvent::SurfaceCreate => MainEvent::SurfaceCreate }); }
    assert_eq!(initialized.get(), 1);
}
#[test] fn stage_only() { check(&[MainEvent::WindowCreate]); }
#[test] fn stage_then_surface_then_peer_stage() { check(&[MainEvent::WindowCreate, MainEvent::SurfaceCreate, MainEvent::WindowCreate]); }
#[test] fn legacy_surface_first() { check(&[MainEvent::SurfaceCreate, MainEvent::WindowCreate, MainEvent::SurfaceCreate]); }
`);
    execFileSync('rustc', ['--edition=2021', '--test', '-A', 'dead_code', harness, '-o', binary], { stdio: 'pipe' });
    execFileSync(binary, { stdio: 'pipe' });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
