import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const tauri = fileURLToPath(new URL('../../target/ohos-sources/tauri/crates/tauri/src/', import.meta.url));

function runRust(source, flags = [], expectedStatus = 0) {
  const directory = mkdtempSync(path.join(tmpdir(), 'rssh-window-commands-'));
  try {
    const file = path.join(directory, 'test.rs');
    const binary = path.join(directory, 'test');
    writeFileSync(file, source);
    execFileSync('rustc', ['--edition=2021', '-A', 'dead_code', ...flags.flatMap((flag) => ['--cfg', flag]), file, '-o', binary]);
    const result = spawnSync(binary, { encoding: 'utf8', timeout: 5000 });
    assert.equal(result.status, expectedStatus, result.stderr);
    return result.stdout.trim();
  } finally { rmSync(directory, { recursive: true, force: true }); }
}

function registeredCommands(flags) {
  const plugin = readFileSync(path.join(tauri, 'window/plugin.rs'), 'utf8');
  // Let rustc apply the actual command-list cfgs. Only command implementations are
  // replaced by their names; the complete registration list comes from Tauri itself.
  const registration = plugin.match(/\.invoke_handler\(crate::generate_handler!\[([\s\S]*?)\]\)/)[1]
    .replace(/#!\[plugin\(window\)\]/, '')
    .replaceAll('target_env = "ohos"', 'ohos')
    .replace(/target_os = "(\w+)"/g, 'os_$1');
  const modules = new Map();
  for (const [, module, command] of registration.matchAll(/(\w+)::(\w+)/g)) {
    if (!modules.has(module)) modules.set(module, new Set());
    modules.get(module).add(command);
  }
  const definitions = Array.from(modules, ([module, commands]) =>
    `mod ${module} { ${Array.from(commands, (command) => `pub const ${command}: &str = "${command}";`).join('\n')} }`).join('\n');
  return new Set(runRust(`#![allow(non_upper_case_globals)]
${definitions}
macro_rules! commands { ($($(#[$attr:meta])* $command:path),* $(,)?) => { vec![$($(#[$attr])* $command),*] }; }
fn main() { let commands: Vec<&str> = commands![${registration}]; print!("{}", commands.join("\\n")); }`, flags).split('\n'));
}

test('HarmonyOS registers complete native window controls without exposing unsupported desktop integrations', () => {
  const commands = registeredCommands(['mobile', 'ohos']);
  for (const command of ['is_minimized', 'is_maximized', 'is_decorated', 'is_maximizable', 'is_minimizable', 'is_closable', 'is_always_on_top', 'maximize', 'unmaximize', 'minimize', 'unminimize', 'set_decorations', 'set_always_on_top', 'start_dragging', 'toggle_maximize', 'internal_toggle_maximize', 'set_title', 'set_size', 'close', 'destroy']) {
    assert.ok(commands.has(command), `${command} must reach the HarmonyOS window dispatcher`);
  }
  for (const command of ['set_effects', 'set_skip_taskbar', 'set_cursor_icon', 'set_fullscreen', 'set_content_protected', 'set_min_size', 'set_max_size', 'set_size_constraints', 'set_theme', 'set_enabled']) {
    assert.equal(commands.has(command), false, `${command} must reject until its native implementation exists`);
  }
});

test('final Tao exit terminates the process even after the Ability bridge has disappeared', () => {
  const source = readFileSync(path.join(tauri, '../../../../tao/src/platform_impl/ohos/mod.rs'), 'utf8');
  const exit = source.match(/(  fn request_exit\(&self, code: i32\) \{[\s\S]*?)\n  pub fn create_proxy/)[1];
  runRust(`struct EventLoop; impl EventLoop { ${exit} } fn main() { EventLoop.request_exit(37); }`, [], 37);
});

test('Android retains mobile commands and desktop retains its full window command group', () => {
  const android = registeredCommands(['mobile', 'os_android']);
  assert.ok(android.has('inner_size'));
  assert.ok(android.has('activity_name'));
  assert.equal(android.has('is_maximized'), false);
  const desktop = registeredCommands(['desktop', 'os_macos']);
  for (const command of ['is_maximized', 'toggle_maximize', 'set_effects', 'set_fullscreen', 'set_theme', 'set_min_size', 'set_badge_label']) assert.ok(desktop.has(command));
});

test('an unhandled HarmonyOS mobile-plugin invoke returns an explicit error instead of leaking its promise', () => {
  const mobile = readFileSync(path.join(tauri, 'plugin/mobile.rs'), 'utf8');
  const command = mobile.match(/#\[cfg\(target_env = "ohos"\)\]\s*([\s\S]*?)\n#\[cfg\(target_os = "ios"\)\]/)[1];
  const output = runRust(`
extern crate self as serde_json;
pub struct Value;
trait Runtime {}
struct Platform;
impl Runtime for Platform {}
struct AppHandle<R>(std::marker::PhantomData<R>);
type PluginResponse = Result<Value, Value>;
#[derive(Debug)] struct ErrorResponse { code: Option<String>, message: Option<String>, data: () }
#[derive(Debug)] enum PluginInvokeError { InvokeRejected(ErrorResponse) }
${command}
fn main() {
  let app = AppHandle::<Platform>(std::marker::PhantomData);
  let result = run_command("window", &app, "unknownCommand", Value, |_| panic!("unsupported command cannot succeed"));
  match result {
    Err(PluginInvokeError::InvokeRejected(error)) => {
      assert_eq!(error.code.as_deref(), Some("UNSUPPORTED_PLUGIN_COMMAND"));
      assert!(error.message.unwrap().contains("window|unknownCommand"));
      print!("rejected");
    }
    Ok(()) => panic!("invoke never completed"),
  }
}`);
  assert.equal(output, 'rejected');
});
