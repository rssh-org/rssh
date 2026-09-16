import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';

const sourceRoot = fileURLToPath(new URL('../../target/ohos-sources/', import.meta.url));

function compile(source, dependencies) {
  const output = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
  const exports = {};
  vm.runInNewContext(output, { exports, require: (name) => dependencies[name], console, Promise, Map, Set, Array, Number, Error });
  return exports;
}

function runtime() {
  class FrameNode {
    children = new Set();
    disposed = false;
    appendChild(child) { this.children.add(child); }
    removeChild(child) { this.children.delete(child); }
    dispose() { this.disposed = true; }
  }
  const source = readFileSync(path.join(sourceRoot, 'ability/native_ability/src/main/ets/components/WindowSurface.ets'), 'utf8');
  const surfaces = compile(source.slice(0, source.indexOf('class WindowRootController')), {});
  const appearance = compile(`import { WindowSurfaceRegistry } from 'surfaces';
export function appear() { ${source.match(/aboutToAppear\(\): void \{([\s\S]*?)\n  \}/)[1]} }`, { surfaces }).appear;
  const entryOptions = vm.runInNewContext(`(${source.match(/@Entry\(([^\n]+)\)/)[1]})`);
  const storageBinding = source.match(/@LocalStorageProp\("([^"]+)"\)\s+surfaceKey:\s*string\s*=\s*"([^"]*)"/);
  const native = { MaximizePresentation: { EXIT_IMMERSIVE: 1 }, WindowStatusType: { MAXIMIZE: 1, MINIMIZE: 2, FLOATING: 3 }, WindowEventType: { WINDOW_ACTIVE: 2, WINDOW_INACTIVE: 3, WINDOW_SHOWN: 1, WINDOW_HIDDEN: 4, WINDOW_DESTROYED: 7 } };
  const events = [];
  const windows = new Map();
  let nextId = 40;
  class LocalStorage {
    values = new Map();
    setOrCreate(key, value) { this.values.set(key, value); }
  }
  class FakeWindow {
    id = nextId++;
    rect = { left: 0, top: 0, width: 1024, height: 768 };
    handlers = new Map();
    destroyed = false;
    operations = [];
    node = new FrameNode();
    on(name, callback) { this.handlers.set(name, callback); }
    off(name) { this.handlers.delete(name); }
    async loadContentByName(route, storage) {
      assert.equal(route, entryOptions.routeName);
      // ArkUI does not bind a loadContentByName LocalStorage to @LocalStorageProp
      // unless the route opts into useSharedStorage (EntryOptions, API 12).
      const boundStorage = entryOptions.useSharedStorage ? storage : new LocalStorage();
      const surfaceKey = boundStorage.values.get(storageBinding[1]) ?? storageBinding[2];
      appearance.call({ surfaceKey, getUIContext: () => this.getUIContext(), controller: { makeNode: () => this.node } });
    }
    getUIContext() { return { owner: this.id, vp2px: (value) => value * 2 }; }
    getWindowProperties() { return { id: this.id, windowRect: this.rect }; }
    getWindowStatus() { return 3; }
    isFocused() { return true; }
    async resize(width, height) { this.rect = { ...this.rect, width, height }; }
    async moveWindowTo(left, top) { this.rect = { ...this.rect, left, top }; }
    setWindowDecorVisible() {}
    async setResizeByDragEnabled() {}
    async setWindowTitle() {}
    setWindowTitleButtonVisible() {}
    async showWindow() { this.operations.push('show'); }
    async minimize() { this.operations.push('minimize'); }
    async maximize(presentation) { assert.equal(presentation, 1); this.operations.push('maximize'); }
    async recover() { this.operations.push('recover'); }
    async restore() { this.operations.push('restore'); }
    async destroyWindow() { this.destroyed = true; this.handlers.get('windowEvent')?.(native.WindowEventType.WINDOW_DESTROYED); }
  }
  const main = new FakeWindow();
  const hostRoot = new FrameNode();
  const context = {
    getRootFrameNode: () => hostRoot,
    sessionId: 'session-a', isActive: () => true,
    getWindow: () => main,
    getWindowStage: () => ({ createSubWindowWithOptions: async (name, options) => { assert.equal(options.decorEnabled, true); assert.equal(options.maximizeSupported, true); const win = new FakeWindow(); windows.set(name.slice(name.lastIndexOf(':native-') + 1), win); return win; } }),
    invokeNativeSync: (_event, _requestType, _responseType, value) => { events.push(value); return { accepted: true }; },
  };
  const managedSource = readFileSync(path.join(sourceRoot, 'ability/plugins/window/src/main/ets/ManagedWindows.ets'), 'utf8');
  const output = ts.transpileModule(managedSource, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
  const exports = {};
  vm.runInNewContext(output, {
    exports, console, LocalStorage, Promise, Map, Array, Number, Error,
    require: (name) => name === '@ohos.window' ? { default: native } : name === '@ohos.deviceInfo' ? { default: { deviceType: '2in1' } } : surfaces,
  });
  return { appearance, owner: surfaces.WindowSurfaceRegistry.owner(context.sessionId, hostRoot), ...surfaces, ManagedWindows: exports.ManagedWindows, FrameNode, FakeWindow, context, windows, main, events, native };
}

function request(id) {
  return { typeName: 'ohos.window.ManagedRequest', value: { windowId: id, title: `window-${id}`, width: 900, height: 600, x: 80, y: 80, visible: true, decorations: true, resizable: true, maximizable: true, minimizable: true, closable: true, maximized: false } };
}

test('closing main cancels pending sub-windows and rejects late creation before destroying the stage', async () => {
  const r = runtime();
  const manager = new r.ManagedWindows(r.context);
  await manager.attach(request(0), r.context);
  const late = new r.FakeWindow();
  let finish;
  r.context.getWindowStage = () => ({ createSubWindowWithOptions: () => new Promise((resolve) => { finish = resolve; }) });
  const pending = manager.attach(request(1), r.context);
  const failed = assert.rejects(pending, /cancelled/);
  const closing = manager.update({ typeName: 'ohos.window.ManagedCommand', value: { windowId: 0, operation: 'destroy' } });
  await assert.rejects(manager.attach(request(2), r.context), /closing/);
  assert.equal(r.main.destroyed, false, 'the stage must survive pending child cleanup');
  finish(late);
  await failed;
  await closing;
  assert.equal(late.destroyed, true);
  assert.equal(r.main.destroyed, true);
  assert.deepEqual(r.events.filter((event) => event.kind === 'destroyed').map((event) => event.windowId), [1, 0]);
});

test('Ability disposal while configuring a sub-window cannot register a late native owner', async () => {
  const r = runtime();
  const manager = new r.ManagedWindows(r.context);
  const late = new r.FakeWindow();
  let reached;
  const configuring = new Promise((resolve) => { reached = resolve; });
  let finish;
  late.setWindowTitle = async () => { reached(); await new Promise((resolve) => { finish = resolve; }); };
  r.context.getWindowStage = () => ({ createSubWindowWithOptions: async () => late });
  const pending = manager.attach(request(1), r.context);
  await configuring;
  await manager.dispose();
  finish();
  await assert.rejects(pending, /cancelled/);
  assert.equal(late.destroyed, true);
  assert.equal(late.handlers.size, 0);
  assert.equal(r.events.length, 0);
});

test('managed windows mount independent UIContexts and native close targets only its owner', async () => {
  const r = runtime();
  const manager = new r.ManagedWindows(r.context);
  await manager.attach(request(0), r.context);
  await manager.attach(request(1), r.context);
  await manager.attach(request(2), r.context);
  const first = r.windows.get('native-1');
  const second = r.windows.get('native-2');
  assert.equal(r.WindowSurfaceRegistry.get(r.owner, 1).getUIContext().owner, first.id);
  assert.equal(r.WindowSurfaceRegistry.get(r.owner, 2).getUIContext().owner, second.id);
  assert.equal(await first.handlers.get('windowWillClose')(), true);
  assert.equal(r.events.at(-1).windowId, 1);
  assert.equal(r.events.at(-1).kind, 'close-requested');
  assert.equal(first.destroyed, false, 'close interception must let Tauri decide');
  await manager.update({ typeName: 'ohos.window.ManagedCommand', value: { windowId: 1, operation: 'destroy' } });
  assert.equal(first.destroyed, true);
  assert.equal(second.destroyed, false);
  assert.equal(r.main.destroyed, false);
  assert.equal(r.events.filter((event) => event.kind === 'destroyed').length, 1);
  assert.equal(r.events.at(-1).windowId, 1);
  await manager.dispose();
});

test('Ability UIContext teardown does not emit user window destruction and a new context can recreate owners', async () => {
  const r = runtime();
  let manager = new r.ManagedWindows(r.context);
  await manager.attach(request(0), r.context);
  await manager.attach(request(1), r.context);
  const original = r.windows.get('native-1');
  await manager.dispose();
  assert.equal(original.destroyed, true);
  assert.equal(r.main.destroyed, false);
  assert.equal(r.events.filter((event) => event.kind === 'destroyed').length, 0);
  manager = new r.ManagedWindows(r.context);
  await manager.attach(request(1), r.context);
  const replacement = r.windows.get('native-1');
  assert.notEqual(replacement.id, original.id);
  assert.equal(r.WindowSurfaceRegistry.get(r.owner, 1).getUIContext().owner, replacement.id);
  await manager.dispose();
});

test('window surface cancellation and session isolation prevent late mounting into another window', async () => {
  const r = runtime();
  const pending = r.WindowSurfaceRegistry.get('old-session', 3);
  let cancel;
  const waiting = pending.wait((callback) => { cancel = callback; return () => {}; });
  cancel();
  await assert.rejects(waiting, /cancelled/);
  const fresh = r.WindowSurfaceRegistry.get('new-session', 3);
  const root = new r.FrameNode();
  fresh.attach({ owner: 33 }, root);
  const child = new r.FrameNode();
  fresh.append('webview', child);
  r.WindowSurfaceRegistry.remove('old-session', 3);
  assert.equal(child.disposed, false);
  assert.equal(root.children.has(child), true);
  r.WindowSurfaceRegistry.remove('new-session', 3);
  assert.equal(child.disposed, true);
});

test('Rust native event registry keeps closure and lifecycle routing window-scoped', () => {
  const directory = mkdtempSync(path.join(tmpdir(), 'rssh-window-events-'));
  try {
    const harness = path.join(directory, 'events.rs');
    const source = path.join(sourceRoot, 'ability/crates/plugin-window/src/events.rs');
    writeFileSync(harness, `#[path = ${JSON.stringify(source)}] mod events;`);
    const binary = path.join(directory, 'events-tests');
    execFileSync('rustc', ['--edition=2021', '--test', '-A', 'dead_code', harness, '-o', binary]);
    execFileSync(binary, ['--test-threads=1']);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});


test('window plugin accepts work after installation and readiness does not replace live owners', async () => {
  const r = runtime();
  class AsyncPluginBase { getContext() { return r.context; } }
  const source = readFileSync(path.join(sourceRoot, 'ability/plugins/window/src/main/ets/WindowPlugin.ets'), 'utf8');
  const { WindowPlugin } = compile(source, {
    '@ohos.window': { default: r.native },
    '@ohos-rs/ability': { AsyncPluginBase },
    './ManagedWindows': { ManagedWindows: r.ManagedWindows },
  });
  const plugin = new WindowPlugin();
  plugin.onInstall(r.context);
  await plugin.invokeAsync('attach-managed-window', request(1), r.context);
  const original = r.windows.get('native-1');
  await plugin.onLifecycle({ kind: 'ui-context-ready' }, r.context);
  await plugin.invokeAsync('update-managed-window', { typeName: 'ohos.window.ManagedCommand', value: { windowId: 1, operation: 'destroy' } }, r.context);
  assert.equal(original.destroyed, true);
  await plugin.onDispose(r.context);
});


test('maximization and minimization use the correct system recovery operation', async () => {
  const r = runtime();
  const manager = new r.ManagedWindows(r.context);
  await manager.attach(request(0), r.context);
  await manager.attach(request(1), r.context);
  for (const id of [0, 1]) {
    for (const [operation, value] of [['maximized', true], ['maximized', false], ['minimized', true], ['minimized', false]]) {
      await manager.update({ typeName: 'ohos.window.ManagedCommand', value: { windowId: id, operation, value } });
    }
  }
  assert.deepEqual(r.main.operations.slice(-4), ['maximize', 'recover', 'minimize', 'restore']);
  assert.deepEqual(r.windows.get('native-1').operations.slice(-4), ['maximize', 'recover', 'minimize', 'show']);
  await manager.dispose();
});


test('disposing a window host during OS creation cancels attachment and destroys the late window', async () => {
  const r = runtime();
  let complete;
  r.context.getWindowStage = () => ({ createSubWindowWithOptions: () => new Promise((resolve) => { complete = resolve; }) });
  const manager = new r.ManagedWindows(r.context);
  const pending = manager.attach(request(1), r.context);
  await manager.dispose();
  const late = new r.FakeWindow();
  complete(late);
  await assert.rejects(pending, /cancelled/);
  assert.equal(late.destroyed, true);
  assert.equal(r.events.length, 0, 'a cancelled creation cannot emit a live owner event');
});

test('surface cleanup isolates module roots sharing an Ability session and releases pending mounts', async () => {
  const r = runtime();
  const first = r.WindowSurfaceRegistry.owner('shared-session', new r.FrameNode());
  const second = r.WindowSurfaceRegistry.owner('shared-session', new r.FrameNode());
  assert.notEqual(first, second);
  const pending = r.WindowSurfaceRegistry.get(first, 1).wait(() => () => {});
  const surviving = r.WindowSurfaceRegistry.get(second, 1);
  surviving.attach({ owner: 91 }, new r.FrameNode());
  r.WindowSurfaceRegistry.clear(first);
  await assert.rejects(pending, /closed before attachment/);
  assert.equal(surviving.getUIContext().owner, 91);
  r.WindowSurfaceRegistry.clear(second);
});


test('native destruction before Ability teardown keeps every logical owner alive', async () => {
  const r = runtime();
  const manager = new r.ManagedWindows(r.context);
  await manager.attach(request(0), r.context);
  await manager.attach(request(1), r.context);
  r.main.handlers.get('windowEvent')(r.native.WindowEventType.WINDOW_DESTROYED);
  r.windows.get('native-1').handlers.get('windowEvent')(r.native.WindowEventType.WINDOW_DESTROYED);
  await manager.dispose();
  assert.equal(r.events.filter((event) => event.kind === 'destroyed').length, 0);
});

test('explicit main-window closure destroys child owners before its bridge can disappear', async () => {
  const r = runtime();
  const manager = new r.ManagedWindows(r.context);
  await manager.attach(request(0), r.context);
  await manager.attach(request(1), r.context);
  await manager.attach(request(2), r.context);
  await manager.update({ typeName: 'ohos.window.ManagedCommand', value: { windowId: 0, operation: 'destroy' } });
  assert.deepEqual(r.events.filter((event) => event.kind === 'destroyed').map((event) => event.windowId), [1, 2, 0]);
  assert.equal(r.main.destroyed, true);
  await manager.dispose();
});


test('an already queued route appearance after Ability disposal cannot throw into ArkUI or remount', async () => {
  const r = runtime();
  const manager = new r.ManagedWindows(r.context);
  const late = new r.FakeWindow();
  let queued;
  const loading = new Promise((resolve) => { queued = resolve; });
  let finish;
  let surfaceKey;
  late.loadContentByName = async (_route, storage) => {
    surfaceKey = storage.values.get('nativeWindowSurface');
    queued();
    await new Promise((resolve) => { finish = resolve; });
  };
  r.context.getWindowStage = () => ({ createSubWindowWithOptions: async () => late });
  const pending = manager.attach(request(1), r.context);
  const cancelled = assert.rejects(pending, /cancelled/);
  await loading;
  await manager.dispose();
  // ArkUI delivers this lifecycle callback independently of the loadContent Promise.
  // An exception here is a process-level runtime error, not a Promise rejection.
  assert.doesNotThrow(() => r.appearance.call({
    surfaceKey,
    getUIContext: () => late.getUIContext(),
    controller: { makeNode: () => { throw new Error('cancelled route must not allocate a frame'); } },
  }));
  assert.throws(() => r.WindowSurfaceRegistry.find(surfaceKey), /Unknown native window surface/);
  finish();
  await cancelled;
  assert.equal(late.destroyed, true);
  assert.equal(r.events.length, 0);
});
