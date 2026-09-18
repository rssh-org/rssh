import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import test from 'node:test';

const source = fileURLToPath(new URL('../../target/ohos-sources/', import.meta.url));
const tauri = path.join(source, 'tauri/crates/tauri');
const read = (name) => readFileSync(path.join(tauri, name), 'utf8');
const channelCommand = 'plugin:__TAURI_CHANNEL__|fetch';

function runtime(os, blocked) {
  const posts = [];
  const fetches = [];
  let nextId = 0;
  const globals = {
    console, Headers, ArrayBuffer, Uint8Array, Uint32Array, Map,
    crypto: { getRandomValues: (values) => { values[0] = ++nextId; return values; } },
    __TAURI_INTERNALS__: {}, ipc: { postMessage: (data) => posts.push(JSON.parse(data)) },
    fetch: (url, options) => { fetches.push({ url, options }); return new Promise(() => {}); },
  };
  globals.window = globals;
  const context = vm.createContext(globals);
  const render = (script) => script
    .replaceAll('__TEMPLATE_os_name__', JSON.stringify(os))
    .replaceAll('__TEMPLATE_protocol_scheme__', '"http"')
    .replaceAll('__TEMPLATE_invoke_key__', '"test-key"')
    .replaceAll('__TEMPLATE_custom_protocol_ipc_blocked__', JSON.stringify(blocked))
    .replaceAll('__TEMPLATE_fetch_channel_data_command__', JSON.stringify(channelCommand))
    .replaceAll('__RAW_process_ipc_message_fn__', read('scripts/process-ipc-message-fn.js'));
  vm.runInContext(render(read('scripts/core.js')), context);
  vm.runInContext(render(read('scripts/ipc-protocol.js')), context);
  globals.__TAURI_INTERNALS__.ipc = globals.__TAURI_INTERNALS__.postMessage;
  return Object.assign(globals.__TAURI_INTERNALS__, { posts, fetches, context });
}

test('OHOS selects postMessage before the first command and preserves binary payloads, errors and headers', async () => {
  const r = runtime('linux', true); // Rust target_os is linux for target_env=ohos.
  const pending = r.invoke('echo', { binary: new Uint8Array([0, 128, 255]), nested: new Map([['key', '值']]) }, { headers: { 'X-Test': 'yes' } });
  assert.equal(r.fetches.length, 0);
  const message = r.posts[0];
  assert.equal(message.options.customProtocolIpcBlocked, true);
  assert.equal(message.options.headers['X-Test'], 'yes');
  assert.deepEqual(message.payload, { binary: [0, 128, 255], nested: { key: '值' } });
  assert.equal(message.__TAURI_INVOKE_KEY__, 'test-key');
  r.runCallback(message.callback, [0, 128, 255]);
  assert.deepEqual(await pending, [0, 128, 255]);
  const failed = r.invoke('fail', new Uint8Array([1, 2, 3]).buffer);
  const rejection = assert.rejects(failed, /expected-error/);
  const failure = r.posts[1];
  assert.deepEqual(failure.payload, [1, 2, 3]);
  r.runCallback(failure.error, new Error('expected-error'));
  await rejection;
  assert.equal(r.callbacks.size, 0);
});

test('large channel data uses the same postMessage transport and completes without a fetch loop', async () => {
  const r = runtime('linux', true);
  const bytes = Array.from({ length: 131072 }, (_, i) => i % 256);
  const pending = r.invoke(channelCommand, null, { headers: { 'Tauri-Channel-Id': '67' } });
  assert.equal(r.fetches.length, 0);
  assert.equal(r.posts.length, 1);
  assert.equal(r.posts[0].options.headers['Tauri-Channel-Id'], '67');
  r.runCallback(r.posts[0].callback, bytes);
  assert.deepEqual(await pending, bytes);
  assert.equal(r.callbacks.size, 0);
});

test('other desktop platforms keep custom IPC and Android keeps its existing channel exception', () => {
  for (const os of ['linux', 'windows', 'macos']) {
    const r = runtime(os, false);
    r.invoke('echo', { value: 3 });
    assert.equal(r.fetches.length, 1);
    assert.equal(r.posts.length, 0);
  }
  const android = runtime('android', false);
  android.invoke('echo');
  assert.equal(android.posts.length, 1);
  android.invoke(channelCommand, null);
  assert.equal(android.fetches.length, 1);
  for (const file of ['src/app.rs', 'src/test/mod.rs']) {
    assert.ok(/custom_protocol_ipc_blocked:\s*cfg!\(target_env = "ohos"\)/.test(read(file)), `${file} must initialize the OHOS transport policy`);
  }
});

test('native custom schemes serve GET and HEAD and reject body methods without touching the unsafe reader', () => {
  const file = path.join(source, 'ability/crates/plugin-webview/src/protocol.rs');
  const native = readFileSync(file, 'utf8');
  const handler = (native.match(/handler\.on_request_start\(([\s\S]*?)\n    \}\);/)[1] + '\n    }')
    .replace('|request, request_handle|', '|request: Request, request_handle: Responder|');
  const directory = mkdtempSync(path.join(tmpdir(), 'rssh-scheme-methods-'));
  try {
    const harness = path.join(directory, 'test.rs');
    const binary = path.join(directory, 'test');
    writeFileSync(harness, `use std::{collections::BTreeMap, sync::{Arc, Mutex}};
struct Request { method: &'static str }
impl Request {
  fn url(&self) -> String { "tauri://localhost/index.html".into() }
  fn method(&self) -> String { self.method.into() }
  fn headers(&self) -> BTreeMap<String, String> { BTreeMap::new() }
  fn is_main_frame(&self) -> bool { true }
  fn http_body_stream(&self) -> Option<Body> { panic!("body stream must never be opened") }
}
struct Body;
impl Body { fn size(&self) -> u64 { 1 } fn read<F: FnMut(Vec<u8>)>(&self, _: usize, _: F) { panic!("unsafe body reader called") } }
struct Responder(Arc<Mutex<Vec<u16>>>);
impl Responder { fn respond(self, status: u16) { self.0.lock().unwrap().push(status); } }
fn responder_for(value: Responder) -> Responder { value }
fn invalid_request_response() -> u16 { 400 }
fn unsupported_request_response() -> u16 { 501 }
fn build_request<'a>(method: &str, _: &str, _: impl Iterator<Item=(&'a String, &'a String)>, body: Vec<u8>) -> Result<(String, Vec<u8>), ()> { Ok((method.into(), body)) }
fn main() {
  for method in ["GET", "HEAD", "POST", "PUT", "DELETE", "OPTIONS", "PATCH"] {
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let called = Arc::new(Mutex::new(Vec::new()));
    let log = called.clone();
    let callback: Arc<dyn Fn(&str, (String, Vec<u8>), bool, Responder)> = Arc::new(move |_, request, _, responder| { log.lock().unwrap().push(request); responder.respond(200); });
    let handle = ${handler};
    assert!(handle(Request { method }, Responder(statuses.clone())));
    if method == "GET" || method == "HEAD" {
      assert_eq!(*statuses.lock().unwrap(), [200]);
      assert_eq!(*called.lock().unwrap(), [(method.to_owned(), Vec::new())]);
    } else {
      assert_eq!(*statuses.lock().unwrap(), [501]);
      assert!(called.lock().unwrap().is_empty());
    }
  }
}`);
    execFileSync('rustc', ['--edition=2021', '-A', 'dead_code', harness, '-o', binary]);
    execFileSync(binary);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
