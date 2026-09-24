import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';

const abilityRoot = process.env.RSSH_ABILITY_ROOT ?? fileURLToPath(new URL('../../target/ohos-sources/ability/', import.meta.url));
const read = (file) => readFileSync(path.join(abilityRoot, file), 'utf8');
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function compile(source, dependencies, globals = {}) {
  const output = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
  const exports = {};
  vm.runInNewContext(output, { exports, require: (name) => {
    if (!(name in dependencies)) throw new Error(`Unexpected dependency ${name}`);
    return dependencies[name];
  }, console, Promise, Map, Set, Array, Number, Error, ...globals });
  return exports;
}

async function runtime() {
  class FrameNode {
    children = new Set();
    disposed = false;
    appendChild(child) { this.children.add(child); }
    removeChild(child) { this.children.delete(child); }
    dispose() { this.disposed = true; }
  }
  class LocalStorage {
    values = new Map();
    setOrCreate(key, value) { this.values.set(key, value); }
  }
  const source = read('native_ability/src/main/ets/components/WindowSurface.ets');
  const surfaces = compile(source.slice(0, source.indexOf('class WindowRootController')), {});
  const appearance = compile(`import { WindowSurfaceRegistry } from 'surfaces';\nexport function appear() { ${source.match(/aboutToAppear\(\): void \{([\s\S]*?)\n  \}/)[1]} }`, { surfaces }).appear;
  const entryOptions = vm.runInNewContext(`(${source.match(/@Entry\(([^\n]+)\)/)[1]})`);
  const storageBinding = source.match(/@LocalStorageProp\("([^"]+)"\)\s+surfaceKey:\s*string\s*=\s*"([^"]*)"/);
  const native = { MaximizePresentation: { EXIT_IMMERSIVE: 1 }, WindowStatusType: { MAXIMIZE: 1, MINIMIZE: 2, FLOATING: 3 }, WindowEventType: { WINDOW_ACTIVE: 2, WINDOW_INACTIVE: 3, WINDOW_SHOWN: 1, WINDOW_HIDDEN: 4, WINDOW_DESTROYED: 7 } };
  const { NativeApplicationStage } = compile(read('native_ability/src/main/ets/ability/NativeApplicationStage.ets'), {
    '@kit.AbilityKit': { AbilityStage: class {} },
  });
  const applicationStage = new NativeApplicationStage();
  const desktopManifest = JSON.parse(readFileSync(new URL('../../gen/ohos/products/desktop/src/main/module.json5', import.meta.url)));
  const peerLaunchType = desktopManifest.module.abilities.find((ability) => ability.name === 'WindowAbility').launchType;
  const instanceKeys = new Map();
  const events = [], launches = [], errors = [], bridgeAttachments = new Map(), detached = [];
  let nextId = 40, emptyCount = 0, firstStageCount = 0;
  class FakeWindow {
    id = nextId++;
    rect = { left: 0, top: 0, width: 1024, height: 768 };
    handlers = new Map();
    destroyed = false;
    foreground = true;
    operations = [];
    node = new FrameNode();
    on(name, callback) { this.handlers.set(name, callback); }
    off(name) { this.handlers.delete(name); }
    async loadContentByName(route, storage) {
      assert.equal(route, entryOptions.routeName);
      const boundStorage = entryOptions.useSharedStorage ? storage : new LocalStorage();
      const surfaceKey = boundStorage.values.get(storageBinding[1]) ?? storageBinding[2];
      appearance.call({ surfaceKey, getUIContext: () => this.getUIContext(), controller: { makeNode: () => this.node } });
    }
    getUIContext() { return { owner: this.id, vp2px: (value) => value * 2 }; }
    getWindowProperties() { return { id: this.id, windowRect: this.rect }; }
    getWindowStatus() { return 3; }
    isFocused() { return !this.destroyed; }
    async resize(width, height) { this.rect = { ...this.rect, width, height }; }
    async moveWindowTo(left, top) { this.rect = { ...this.rect, left, top }; }
    setWindowDecorVisible() {}
    async setResizeByDragEnabled() {}
    async setWindowTitle() {}
    setWindowTitleButtonVisible() {}
    async showWindow() { this.operations.push('show'); }
    async minimize() { this.foreground = false; this.operations.push('minimize'); }
    async maximize(presentation) { assert.equal(presentation, 1); this.operations.push('maximize'); }
    async recover() { this.operations.push('recover'); }
    async restore() {
      if (!this.foreground) throw new Error('restore requires its UIAbility in the foreground');
      this.operations.push('restore');
    }
    async raiseToAppTop() { throw new Error('1300004: raiseToAppTop supports only subwindows'); }
    async destroyWindow() { this.destroyed = true; this.handlers.get('windowEvent')?.(native.WindowEventType.WINDOW_DESTROYED); }
  }
  const bridge = {
    attachApplicationWindow: async (_session, _module, id, context, stage, uiContext, root) => bridgeAttachments.set(id, { context, stage, uiContext, root }),
    detachApplicationWindow: (_session, _module, id) => { detached.push(id); bridgeAttachments.delete(id); },
  };
  const { ApplicationWindows } = compile(read('native_ability/src/main/ets/runtime/ApplicationWindows.ets'), {
    '../bridge/BridgeHost': { BridgeHostRegistry: bridge }, '../components/WindowSurface': surfaces,
  }, { LocalStorage });
  const application = new ApplicationWindows('session-a', 'rssh_lib', 'RsshWindowAbility', async () => { emptyCount++; }, async () => { firstStageCount++; });
  const main = new FakeWindow();
  const windows = new Map([[0, main]]);
  const abilities = new Map();
  const peers = new Map();
  let startBehavior;
  const makeStage = (win) => ({ getMainWindowSync: () => win, loadContentByName: (...args) => win.loadContentByName(...args) });
  const makeContext = (id, nonce, win) => ({
    applicationInfo: { name: 'com.rssh.app' }, abilityInfo: { moduleName: 'desktop', name: id === 0 ? 'EntryAbility' : 'RsshWindowAbility' },
    startAbility: async (want) => { launches.push({ issuer: id, want }); return await startBehavior(want); },
    terminateSelf: async () => { await win.destroyWindow(); queueMicrotask(() => { void application.detach(id, nonce); }); },
  });
  const initialContext = makeContext(0, 'initial', main);
  await application.attach(0, 'initial', initialContext, makeStage(main));
  const { NativeApplicationAbility } = compile(read('native_ability/src/main/ets/ability/NativeApplicationAbility.ets'), {
    '@kit.AbilityKit': { UIAbility: class {} },
    './NativeApplication': { NativeApplication: { find: async (session, module) => {
      assert.equal(session, 'session-a'); assert.equal(module, 'rssh_lib'); return { windows: application };
    }, acquire: () => { throw new Error('a reserved peer must not initialize the native application'); } } },
  }, { console: { error: (...args) => errors.push(args.join(' ')) } });
  const createPeer = (want, win = new FakeWindow()) => {
    const { nativeWindowId: id, nativeWindowNonce: nonce } = want.parameters;
    const ability = new NativeApplicationAbility();
    ability.moduleName = 'rssh_lib';
    ability.context = makeContext(id, nonce, win);
    peers.set(id, ability.context);
    instanceKeys.set(id, applicationStage.onAcceptWant(want));
    windows.set(id, win);
    abilities.set(id, ability);
    ability.onCreate(want, {});
    return { ability, win };
  };
  const launch = async (want, win = new FakeWindow()) => {
    const { ability } = createPeer(want, win);
    ability.onWindowStageCreate(makeStage(win));
    await ability.stageWork;
    return { ability, win };
  };
  const start = async (want) => {
    const id = want.parameters?.nativeWindowId ?? 0;
    const existing = application.hosts.get(id);
    if (existing && (id === 0 || (peerLaunchType === 'specified' && instanceKeys.get(id) === applicationStage.onAcceptWant(want)))) {
      if (existing.win.destroyed) throw new Error('UIAbility is already destroyed');
      existing.win.foreground = true;
      existing.win.operations.push('ability-focus');
      return;
    }
    queueMicrotask(() => { void launch(want).catch((error) => errors.push(String(error))); });
  };
  startBehavior = start;
  const context = {
    sessionId: 'session-a', isActive: () => true, onCancel: () => () => {},
    getRootFrameNode: () => main.node, getWindow: () => main,
    getWindowStage: () => { throw new Error('independent windows must not create sub-windows'); },
    invokeNativeSync: (_event, _requestType, _responseType, value) => { events.push(value); return { accepted: true }; },
  };
  const { ManagedWindows } = compile(read('plugins/window/src/main/ets/ManagedWindows.ets'), {
    '@ohos.window': { default: native }, '@ohos.deviceInfo': { default: { deviceType: '2in1' } },
    '@ohos-rs/ability': { ...surfaces, ApplicationWindows },
  });
  const manager = new ManagedWindows(context);
  return { ...surfaces, ApplicationWindows, application, manager, ManagedWindows, NativeApplicationAbility, FrameNode, FakeWindow,
    appearance, owner: 'session-a', context, windows, main, events, native, launches, errors, abilities, peers, bridgeAttachments, detached,
    launch, createPeer, start, applicationStage, desktopManifest, setStart: (behavior) => { startBehavior = behavior; }, makeStage,
    emptyCount: () => emptyCount, firstStageCount: () => firstStageCount,
    dispose: async () => { await manager.dispose(); await application.dispose(); },
  };
}

function request(id) {
  return { typeName: 'ohos.window.ManagedRequest', value: { windowId: id, title: `window-${id}`, width: 900, height: 600, x: 80, y: 80, visible: true, decorations: true, resizable: true, maximizable: true, minimizable: true, closable: true, maximized: false } };
}
const command = (windowId, operation, value) => ({ typeName: 'ohos.window.ManagedCommand', value: { windowId, operation, value } });
function cancellable() {
  let active = true;
  const listeners = new Set();
  return { isActive: () => active, onCancel: (callback) => { listeners.add(callback); return () => listeners.delete(callback); }, cancel: () => { active = false; for (const listener of [...listeners]) listener(); } };
}

test('native configuration failure settles existing and late surface waiters and terminates only its Ability', async () => {
  const r = await runtime();
  const child = new r.FakeWindow();
  const configuring = deferred(), finish = deferred();
  child.setWindowTitle = () => { configuring.resolve(); return finish.promise; };
  r.setStart(async (want) => { void r.launch(want, child); });
  const attaching = r.manager.attach(request(1), r.context);
  const rejected = assert.rejects(attaching, /native title failure/);
  await configuring.promise;
  const surface = r.WindowSurfaceRegistry.get(r.owner, 1);
  let ready = false;
  const waiting = surface.wait(() => () => {}).then(() => { ready = true; });
  const waitRejected = assert.rejects(waiting, /native title failure/);
  await Promise.resolve();
  assert.equal(ready, false, 'route mounting alone is not native configuration completion');
  finish.reject(new Error('native title failure'));
  await rejected; await waitRejected;
  assert.equal(child.destroyed, true);
  assert.equal(r.main.destroyed, false);
  await assert.rejects(r.WindowSurfaceRegistry.get(r.owner, 1).wait(() => () => {}), /native title failure/);
  await r.manager.update(command(1, 'destroy'));
  assert.equal(r.WindowSurfaceRegistry.has(r.WindowSurfaceRegistry.key(r.owner, 1)), false);
  await r.dispose();
});

test('private specified start failure settles surface waiters and leaves the existing window live', async () => {
  const r = await runtime();
  r.setStart(async () => { throw new Error('OS window limit'); });
  const waiting = r.WindowSurfaceRegistry.get(r.owner, 1).wait(() => () => {});
  const rejected = assert.rejects(waiting, /OS window limit/);
  await assert.rejects(r.manager.attach(request(1), r.context), /OS window limit/);
  await rejected;
  await assert.rejects(r.WindowSurfaceRegistry.get(r.owner, 1).wait(() => () => {}), /OS window limit/);
  assert.equal(r.main.destroyed, false);
  assert.equal(r.emptyCount(), 0);
  await r.dispose();
});

test('surface readiness requires route attachment and native completion in either order', async () => {
  const r = await runtime();
  for (const attachFirst of [true, false]) {
    const surface = new r.WindowSurface();
    const attach = () => surface.attach({ owner: 7 }, new r.FrameNode());
    const complete = () => surface.markReady();
    (attachFirst ? attach : complete)();
    let ready = false;
    const waiting = surface.wait(() => () => {}).then(() => { ready = true; });
    await Promise.resolve(); assert.equal(ready, false);
    (attachFirst ? complete : attach)();
    await waiting; assert.equal(ready, true); surface.dispose();
  }
  await r.dispose();
});

test('closing the first window preserves a peer whose UIAbility launch is still pending', async () => {
  const r = await runtime();
  await r.manager.attach(request(0), r.context);
  const launched = deferred();
  r.setStart(async (want) => { launched.resolve(want); });
  const pending = r.manager.attach(request(1), r.context);
  const want = await launched.promise;
  await r.manager.update(command(0, 'destroy'));
  assert.equal(r.main.destroyed, true);
  assert.equal(r.emptyCount(), 0, 'a reserved peer keeps the application alive');
  await r.launch(want);
  await pending;
  assert.equal(r.windows.get(1).destroyed, false);
  assert.equal(r.WindowSurfaceRegistry.get(r.owner, 1).getUIContext().owner, r.windows.get(1).id);
  assert.deepEqual(r.events.filter((event) => event.kind === 'destroyed').map((event) => event.windowId), [0]);
  assert.equal(r.firstStageCount(), 1);
  await r.dispose();
});

test('surviving windows start another independent Ability after the first window closes', async () => {
  const r = await runtime();
  for (const id of [0, 1]) await r.manager.attach(request(id), r.context);
  await r.manager.update(command(0, 'destroy'));
  await r.manager.attach(request(2), r.context);
  const launch = r.launches.at(-1);
  assert.equal(launch.issuer, 1);
  assert.equal(launch.want.bundleName, 'com.rssh.app');
  assert.equal(launch.want.moduleName, 'desktop');
  assert.equal(launch.want.abilityName, 'RsshWindowAbility');
  assert.equal(launch.want.parameters.nativeWindowId, 2);
  assert.equal(launch.want.parameters.nativeApplicationSession, 'session-a');
  assert.ok(launch.want.parameters.nativeWindowNonce);
  assert.equal(r.windows.get(1).destroyed, false);
  assert.equal(r.windows.get(2).destroyed, false);
  assert.equal(r.firstStageCount(), 1, 'another UIAbility must not initialize Tauri again');
  await r.dispose();
});

test('native close targets only the selected owner and every window has its own UIContext', async () => {
  const r = await runtime();
  for (const id of [0, 1, 2]) await r.manager.attach(request(id), r.context);
  for (const id of [0, 1, 2]) {
    assert.equal(r.WindowSurfaceRegistry.get(r.owner, id).getUIContext().owner, r.windows.get(id).id);
    assert.equal(r.bridgeAttachments.get(id).root, r.windows.get(id).node);
  }
  const first = r.windows.get(1);
  assert.equal(await first.handlers.get('windowWillClose')(), true);
  assert.equal(r.events.at(-1).windowId, 1);
  assert.equal(r.events.at(-1).kind, 'close-requested');
  assert.equal(first.destroyed, false);
  await r.manager.update(command(1, 'destroy'));
  assert.equal(first.destroyed, true);
  assert.equal(r.main.destroyed, false);
  assert.equal(r.windows.get(2).destroyed, false);
  assert.deepEqual(r.events.filter((event) => event.kind === 'destroyed').map((event) => event.windowId), [1]);
  await r.manager.update(command(0, 'destroy'));
  assert.equal(r.emptyCount(), 0);
  await r.manager.update(command(2, 'destroy'));
  assert.equal(r.emptyCount(), 1, 'only the last live and pending window releases the application');
  await r.dispose();
});

test('cancelling a reserved creation rejects a late Ability and its route cannot mount', async () => {
  const r = await runtime();
  const launched = deferred();
  r.setStart(async (want) => { launched.resolve(want); });
  const call = cancellable();
  const pending = r.manager.attach(request(1), call);
  const rejected = assert.rejects(pending, /cancelled/);
  const want = await launched.promise;
  call.cancel();
  await rejected;
  const { win } = await r.launch(want);
  assert.equal(win.destroyed, true, 'late specified instance terminates itself');
  assert.equal(r.bridgeAttachments.has(1), false);
  assert.equal(r.windows.get(0).destroyed, false);
  assert.doesNotThrow(() => r.appearance.call({
    surfaceKey: r.WindowSurfaceRegistry.key(r.owner, 1),
    getUIContext: () => { throw new Error('cancelled route must not allocate a frame'); },
  }));
  assert.ok(r.errors.some((error) => /cancelled application window reservation/.test(error)));
  await r.dispose();
});

test('an incorrect creation nonce cannot claim a pending peer', async () => {
  const r = await runtime();
  const launched = deferred();
  r.setStart(async (want) => { launched.resolve(want); });
  const pending = r.manager.attach(request(1), r.context);
  const want = await launched.promise;
  const invalid = { ...want, parameters: { ...want.parameters, nativeWindowNonce: 'stale' } };
  const stale = await r.launch(invalid);
  assert.equal(stale.win.destroyed, true);
  assert.equal(r.bridgeAttachments.has(1), false);
  const valid = await r.launch(want);
  await pending;
  assert.equal(valid.win.destroyed, false);
  assert.equal(r.bridgeAttachments.get(1).uiContext.owner, valid.win.id);
  await r.dispose();
});

test('a peer destroyed before WindowStage creation releases its reservation and allows final shutdown', async () => {
  for (const closeFirstWindowBeforePeer of [false, true]) {
    const r = await runtime();
    await r.manager.attach(request(0), r.context);
    const launched = deferred();
    r.setStart(async (want) => { launched.resolve(want); });
    const pending = r.manager.attach(request(1), r.context);
    const rejected = assert.rejects(pending, /closed before window attachment/);
    const want = await launched.promise;
    const { ability } = r.createPeer(want);
    await ability.creating;
    assert.equal(r.bridgeAttachments.has(1), false, 'no WindowStage or mounted host exists yet');
    if (closeFirstWindowBeforePeer) {
      await r.manager.update(command(0, 'destroy'));
      assert.equal(r.emptyCount(), 0, 'the pending peer keeps the application alive');
    }
    await ability.onDestroy();
    await rejected;
    await assert.rejects(r.WindowSurfaceRegistry.get(r.owner, 1).wait(() => () => {}), /closed before window attachment/);
    if (!closeFirstWindowBeforePeer) {
      assert.equal(r.emptyCount(), 0, 'the surviving first window keeps the application alive');
      await r.manager.update(command(0, 'destroy'));
    }
    assert.equal(r.emptyCount(), 1, 'no unresolved peer reservation can block application shutdown');
    await r.dispose();
  }
});

test('application shutdown rejects pending creation and a later Ability cannot revive it', async () => {
  const r = await runtime();
  const launched = deferred();
  r.setStart(async (want) => { launched.resolve(want); });
  const pending = r.manager.attach(request(1), r.context);
  const rejected = assert.rejects(pending, /shutting down/);
  const want = await launched.promise;
  await r.dispose();
  await rejected;
  const late = await r.launch(want);
  assert.equal(late.win.destroyed, true);
  assert.equal(r.bridgeAttachments.size, 0);
  assert.equal(r.events.length, 0);
});

test('native destruction and later Ability teardown report one closure for that owner', async () => {
  const r = await runtime();
  for (const id of [0, 1]) await r.manager.attach(request(id), r.context);
  const peer = r.windows.get(1);
  await peer.destroyWindow();
  await r.application.detach(1, r.launches[0].want.parameters.nativeWindowNonce);
  assert.deepEqual(r.events.filter((event) => event.kind === 'destroyed').map((event) => event.windowId), [1]);
  assert.equal(r.main.destroyed, false);
  await r.dispose();
});

test('maximization and minimization restore every independent main window', async () => {
  const r = await runtime();
  for (const id of [0, 1]) {
    await r.manager.attach(request(id), r.context);
    for (const [operation, value] of [['maximized', true], ['maximized', false], ['minimized', true], ['minimized', false]]) {
      await r.manager.update(command(id, operation, value));
    }
    assert.deepEqual(r.windows.get(id).operations.slice(-4), ['maximize', 'recover', 'minimize', 'ability-focus']);
  }
  await r.dispose();
});

test('window surface cancellation and session isolation prevent mounting into another window', async () => {
  const r = await runtime();
  const pending = r.WindowSurfaceRegistry.get('old-session', 3);
  let cancel;
  const waiting = pending.wait((callback) => { cancel = callback; return () => {}; });
  cancel(); await assert.rejects(waiting, /cancelled/);
  const fresh = r.WindowSurfaceRegistry.get('new-session', 3);
  const root = new r.FrameNode();
  fresh.attach({ owner: 33 }, root);
  const child = new r.FrameNode();
  fresh.append('webview', child);
  r.WindowSurfaceRegistry.remove('old-session', 3);
  assert.equal(child.disposed, false); assert.equal(root.children.has(child), true);
  r.WindowSurfaceRegistry.remove('new-session', 3);
  assert.equal(child.disposed, true);
  await r.dispose();
});

test('legacy module roots still isolate surface cleanup when sharing an Ability session', async () => {
  const r = await runtime();
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
  await r.dispose();
});

test('window plugin readiness does not replace live application window owners', async () => {
  const r = await runtime();
  class AsyncPluginBase { getContext() { return r.context; } }
  const { WindowPlugin } = compile(read('plugins/window/src/main/ets/WindowPlugin.ets'), {
    '@ohos.window': { default: r.native }, '@ohos-rs/ability': { AsyncPluginBase },
    './ManagedWindows': { ManagedWindows: r.ManagedWindows },
  });
  const plugin = new WindowPlugin();
  plugin.onInstall(r.context);
  await plugin.invokeAsync('attach-managed-window', request(1), r.context);
  const original = r.windows.get(1);
  await plugin.onLifecycle({ kind: 'ui-context-ready' }, r.context);
  await plugin.invokeAsync('update-managed-window', command(1, 'destroy'), r.context);
  assert.equal(original.destroyed, true);
  await plugin.onDispose(r.context);
  await r.dispose();
});

test('Rust native event registry keeps closure and lifecycle routing window-scoped', () => {
  const directory = mkdtempSync(path.join(tmpdir(), 'rssh-window-events-'));
  try {
    const harness = path.join(directory, 'events.rs');
    const source = path.join(abilityRoot, 'crates/plugin-window/src/events.rs');
    writeFileSync(harness, `#[path = ${JSON.stringify(source)}] mod events;`);
    const binary = path.join(directory, 'events-tests');
    execFileSync('rustc', ['--edition=2021', '--test', '-A', 'dead_code', harness, '-o', binary]);
    execFileSync(binary, ['--test-threads=1']);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});


test('reopening Entry foregrounds the same minimized peer UIAbility after the first window closes', async () => {
  const r = await runtime();
  for (const id of [0, 1]) await r.manager.attach(request(id), r.context);
  await r.application.close(0);
  const peer = r.windows.get(1);
  await peer.minimize();
  const requester = { async startAbility(want) { r.launches.push({ issuer: 'new-entry', want }); await r.start(want); } };
  await r.application.focusExisting(requester);
  assert.equal(peer.foreground, true);
  assert.equal(r.launches.at(-1).issuer, 'new-entry');
  assert.equal(r.launches.at(-1).want.parameters.nativeWindowNonce, r.launches[0].want.parameters.nativeWindowNonce);
  assert.equal(r.windows.size, 2, 'focus must not create another peer');
  assert.equal(r.application.hosts.size, 1);
  assert.equal(r.emptyCount(), 0);
  await r.dispose();
});

test('focus commands use Ability activation for both Entry and peer main windows', async () => {
  const r = await runtime();
  for (const id of [0, 1]) {
    await r.manager.attach(request(id), r.context);
    await r.windows.get(id).minimize();
    await r.manager.update(command(id, 'focus'));
    assert.equal(r.windows.get(id).foreground, true);
    assert.equal(r.windows.get(id).operations.at(-1), 'ability-focus');
  }
  assert.equal(r.windows.size, 2);
  await r.dispose();
});

test('focus skips a closing instance but propagates failures when every instance rejects activation', async () => {
  const r = await runtime();
  for (const id of [0, 1]) await r.manager.attach(request(id), r.context);
  let attempts = 0;
  const requester = { async startAbility(want) {
    attempts++;
    if (!want.parameters) throw new Error('entry has already closed');
    await r.start(want);
  } };
  await r.application.focusExisting(requester);
  assert.equal(attempts, 2);
  await assert.rejects(r.application.focusExisting({ async startAbility() { throw new Error('SDK activation refused'); } }), /SDK activation refused/);
  assert.equal(r.application.hosts.size, 2, 'activation failures must not destroy existing windows');
  await r.dispose();
});


test('desktop module installs the specified instance stage and keeps reservation identities distinct', async () => {
  const r = await runtime();
  const manifest = r.desktopManifest.module;
  const root = new URL('../../gen/ohos/products/desktop/src/main/', import.meta.url);
  const { default: ApplicationStage } = compile(readFileSync(new URL(manifest.srcEntry, root), 'utf8'), {
    '@rssh/runtime': { NativeApplicationStage: r.applicationStage.constructor },
  });
  const stage = new ApplicationStage();
  const peer = manifest.abilities.find((ability) => ability.name === 'WindowAbility');
  assert.equal(peer.launchType, 'specified');
  assert.equal(peer.exported, false);
  assert.equal(peer.removeMissionAfterTerminate, true);
  const key = (nativeWindowNonce) => stage.onAcceptWant({ parameters: { nativeWindowNonce } });
  assert.equal(key('session-a:1'), key('session-a:1'));
  assert.notEqual(key('session-a:1'), key('session-a:2'));
  assert.notEqual(key('session-a:1'), key('session-b:1'));
  assert.equal(key(undefined), '', 'malformed launches still reach the Ability identity validation');
  await r.dispose();
});
