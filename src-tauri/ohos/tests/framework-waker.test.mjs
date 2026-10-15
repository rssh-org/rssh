import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

// Import the real framework waker and its creation method. Only the N-API
// scheduling boundary is fake; no native library, WebView or device is loaded.
test('framework wakers resolve the current lifecycle callback when waking', (context) => {
  const directory = mkdtempSync(path.join(tmpdir(), 'rssh-framework-waker-'));
  try {
    const sourceDirectory = fileURLToPath(new URL('../../target/ohos-sources/ability/crates/ability/src/', import.meta.url));
    const source = path.join(sourceDirectory, 'waker.rs');
    const appSource = readFileSync(path.join(sourceDirectory, 'app.rs'), 'utf8');
    const createWaker = appSource.match(/pub fn create_waker\(&self\) -> OpenHarmonyWaker \{[\s\S]*?\n    \}/)?.[0];
    assert.ok(createWaker, 'the framework app must expose its waker factory');
    const harness = path.join(directory, 'waker-tests.rs');
    const binary = path.join(directory, process.platform === 'win32' ? 'waker-tests.exe' : 'waker-tests');
    writeFileSync(harness, `
extern crate self as napi_ohos;

pub mod threadsafe_function {
    use std::{marker::PhantomData, sync::Arc};

    pub enum ThreadsafeFunctionCallMode { NonBlocking }
    pub struct ThreadsafeFunction<T, R> {
        callback: Arc<dyn Fn() + Send + Sync>,
        marker: PhantomData<fn(T) -> R>,
    }
    impl ThreadsafeFunction<(), ()> {
        pub fn new(callback: impl Fn() + Send + Sync + 'static) -> Self {
            Self { callback: Arc::new(callback), marker: PhantomData }
        }
        pub fn call(&self, _: Result<(), ()>, _: ThreadsafeFunctionCallMode) {
            (self.callback)();
        }
    }
}

#[path = ${JSON.stringify(source)}]
mod waker;
use waker::{OpenHarmonyWaker, WAKER};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use threadsafe_function::ThreadsafeFunction;

struct App;
impl App { ${createWaker} }

fn install(counter: Arc<AtomicUsize>) {
    *WAKER.write().unwrap() = Some(Arc::new(ThreadsafeFunction::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })));
}

#[test]
fn waker_created_before_registration_uses_later_callback() {
    *WAKER.write().unwrap() = None;
    let early = App.create_waker();
    let counter = Arc::new(AtomicUsize::new(0));
    install(counter.clone());
    std::thread::spawn(move || early.wake()).join().unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[test]
fn cloned_waker_uses_replacement_lifecycle_callback() {
    let old = Arc::new(AtomicUsize::new(0));
    install(old.clone());
    let original = App.create_waker();
    let cloned = original.clone();
    let current = Arc::new(AtomicUsize::new(0));
    install(current.clone());
    std::thread::spawn(move || { original.wake(); cloned.wake(); }).join().unwrap();
    assert_eq!(old.load(Ordering::SeqCst), 0);
    assert_eq!(current.load(Ordering::SeqCst), 2);
}

#[test]
fn waking_before_registration_is_safe() {
    *WAKER.write().unwrap() = None;
    let waker = App.create_waker();
    std::thread::spawn(move || waker.wake()).join().unwrap();
}

#[test]
fn scheduling_does_not_hold_the_lifecycle_slot_lock() {
    *WAKER.write().unwrap() = Some(Arc::new(ThreadsafeFunction::new(|| {
        assert!(WAKER.try_write().is_ok(), "the slot lock must be released before scheduling");
    })));
    App.create_waker().wake();
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
