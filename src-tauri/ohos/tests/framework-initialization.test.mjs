import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import test from 'node:test';

const sourceRoot = fileURLToPath(new URL('../../target/ohos-sources/', import.meta.url));

test('Wry injects real Tauri bootstrap and metadata in order before plugin scripts', () => {
  const manager = readFileSync(path.join(sourceRoot, 'tauri/crates/tauri/src/manager/webview.rs'), 'utf8');
  const bootstrap = manager.match(/all_initialization_scripts\.push\(main_frame_script\(\s*r"([\s\S]*?)"/)[1];
  const metadata = manager.match(/main_frame_script\(format!\(\s*r#"([\s\S]*?'metadata'[\s\S]*?)"#/)[1]
    .replaceAll('{current_window_label}', '"main"').replaceAll('{current_webview_label}', '"main"')
    .replaceAll('{{', '{').replaceAll('}}', '}');
  const scripts = [
    [bootstrap, true], [metadata, true],
    ['throw new Error("isolated failure");', true],
    ['window.pluginOwner = window.__TAURI_INTERNALS__.metadata.currentWindow.label; // no trailing newline', true],
    ['var sharedAcrossFrames = 73;', false],
    ['window.everyFrame = sharedAcrossFrames;', false],
  ];
  // The old ArkWeb API executes ScriptItems lexicographically. Reproduce the exact
  // failure before checking that Wry now submits one ordered ScriptItem.
  const legacy = {};
  legacy.window = legacy;
  legacy.top = legacy;
  assert.throws(() => {
    for (const script of [bootstrap, metadata].sort()) vm.runInNewContext(script, legacy);
  }, /Object.defineProperty called on non-object/);

  const directory = mkdtempSync(path.join(tmpdir(), 'rssh-wry-init-'));
  try {
    const source = path.join(sourceRoot, 'wry/src/ohos/initialization.rs');
    const harness = path.join(directory, 'init.rs');
    const binary = path.join(directory, 'init');
    writeFileSync(harness, `#[path = ${JSON.stringify(source)}] mod initialization;
fn main() { print!("{}", initialization::ordered_script(vec![${scripts.map(([script, main]) => `(r#####"${script}"#####.into(), ${main})`).join(',')}])) }`);
    execFileSync('rustc', ['--edition=2021', harness, '-o', binary]);
    const injected = execFileSync(binary, { encoding: 'utf8' });
    const errors = [];
    const main = { console: { error: (...args) => errors.push(args) } };
    main.window = main;
    main.top = main;
    vm.runInNewContext(injected, main);
    assert.equal(main.pluginOwner, 'main');
    assert.equal(main.__TAURI_INTERNALS__.metadata.currentWebview.label, 'main');
    assert.equal(main.everyFrame, 73);
    assert.equal(errors.length, 1, 'a failed script must not prevent metadata/plugin setup');
    const child = { console: main.console, top: main };
    child.window = child;
    vm.runInNewContext(injected, child);
    assert.equal(child.__TAURI_INTERNALS__, undefined, 'main-frame-only scripts must not run in frames');
    assert.equal(child.everyFrame, 73, 'all-frame scripts still execute in frames');
    assert.equal(errors.length, 1);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
