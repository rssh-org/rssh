import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';
import { setImmediate as nextTurn } from 'node:timers/promises';
import test from 'node:test';
import ts from 'typescript';

// Execute the patched framework; only SDK, bridge transport and native module are
// replaced. No native device, hidden Ability or application data is involved.
const framework = process.env.RSSH_ABILITY_ROOT ?? fileURLToPath(new URL('../../target/ohos-sources/ability/native_ability/src/main/ets/', import.meta.url));
function gate() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

function harness() {
  const cache = new Map();
  const calls = [];
  const windows = [];
  const errors = [];
  let loadGate;
  let failAttach = false;
  let failActivation = false;
  let failFocus = false;
  let mountGate;
  const lifecycle = {
    bridgePlugins: [],
    windowStageEventCallback: {
      onAbilityCreate: () => calls.push('ability-create'),
      onAbilityDestroy: () => calls.push('ability-destroy'),
      onWindowStageCreate: () => calls.push('stage-create'),
      onWindowStageDestroy: () => calls.push('stage-destroy'),
      onAbilitySaveState: () => 'application-state',
    },
    environmentCallback: { onConfigurationUpdated() {}, onMemoryLevel() {} },
  };
  const native = {
    init(_bindings, owner) { calls.push(['init', owner]); return lifecycle; },
    disposeBridge: (owner) => calls.push(['dispose-bridge', owner]),
    disposeAllRenders: () => calls.push('dispose-renders'),
    onBridgeSyncEvent() {},
    onBridgeLifecycle() {},
  };
  class ApplicationWindows {
    constructor(sessionId, moduleName, peer, onEmpty, onFirstStage) {
      Object.assign(this, { sessionId, moduleName, peer, onEmpty, onFirstStage });
      this.attached = new Map();
      this.first = false;
      windows.push(this);
    }
    async attach(id, nonce, _context, stage) {
      if (!this.first) { this.first = true; await this.onFirstStage(stage); }
      if (failAttach) { failAttach = false; throw new Error('window attach failed'); }
      this.attached.set(id, nonce);
      calls.push(['attach', id]);
      if (mountGate) await mountGate.promise;
    }
    async detach(id, nonce) {
      if (this.attached.get(id) === nonce) this.attached.delete(id);
      calls.push(['detach', id]);
      mountGate?.resolve();
      if (this.attached.size === 0) await this.onEmpty();
    }
    async focusExisting(context) {
      assert.ok(context.filesDir, 'reactivation must use the new Entry caller context');
      calls.push('focus-existing');
      if (failFocus) throw new Error('SDK refused to activate existing windows');
    }
    async dispose() { this.attached.clear(); calls.push('dispose-windows'); }
  }
  const BridgeHostRegistry = {
    async prepare() { calls.push('prepare'); return 'session-1'; },
    configurePlugins() { calls.push('configure'); },
    attachApplicationLifecycle() { calls.push('attach-lifecycle'); },
    attachEventSink() { calls.push('attach-sinks'); },
    async activateAbility() { calls.push('activate'); if (failActivation) throw new Error('plugin installation failed'); },
    async setWindowStage() { calls.push('set-stage'); },
    async emitLifecycle(_session, _module, event) { calls.push(['lifecycle', event.kind]); },
    beginClosing() { calls.push('begin-closing'); },
    async clearWindowStage() { calls.push('clear-stage'); },
    async dispose() { calls.push('dispose-host'); },
  };
  const sdk = {
    '@kit.AbilityKit': { UIAbility: class {}, AbilityConstant: { OnSaveResult: { RECOVERY_AGREE: 1 } } },
    '@ohos.window': { default: {} },
    '@ohos.arkui.node': {},
  };
  function load(relative) {
    const filename = path.resolve(framework, relative);
    if (cache.has(filename)) return cache.get(filename);
    const exports = {};
    cache.set(filename, exports);
    const source = readFileSync(filename, 'utf8');
    const code = ts.transpileModule(source, {
      compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
    }).outputText;
    runInNewContext(code, {
      exports,
      require(id) {
        if (id in sdk) return sdk[id];
        if (id.endsWith('/bridge/BridgeHost')) return { BridgeHostRegistry };
        if (id.endsWith('/runtime/NativeModuleLoader')) return {
          NativeModuleLoader: { async load() { if (loadGate) await loadGate; return native; } },
        };
        if (id.endsWith('/runtime/ApplicationWindows')) return { ApplicationWindows };
        if (id.startsWith('.')) return load(path.relative(framework, path.resolve(path.dirname(filename), id + '.ets')));
        throw new Error(`Unexpected dependency: ${id}`);
      },
      console: { error: (...args) => errors.push(args.join(' ')) },
    }, { filename });
    return exports;
  }
  const { NativeApplicationAbility } = load('ability/NativeApplicationAbility.ets');
  function ability(parameters = {}) {
    const instance = new NativeApplicationAbility();
    instance.moduleName = 'rssh_lib';
    instance.peerAbilityName = 'WindowAbility';
    instance.context = {
      filesDir: '/application/files', config: { language: 'zh' },
      async terminateSelf() { calls.push(['terminate', instance.windowId]); },
    };
    instance.onCreate({ parameters }, {});
    return instance;
  }
  function peer(id = 1) {
    return ability({ nativeApplicationSession: 'session-1', nativeWindowId: id, nativeWindowNonce: `nonce-${id}` });
  }
  async function stage(instance) {
    instance.onWindowStageCreate({});
    await instance.stageWork;
  }
  return {
    ability, peer, stage, calls, windows, errors,
    blockLoad: (promise) => { loadGate = promise; },
    blockMount: () => { mountGate = gate(); },
    rejectAttach: () => { failAttach = true; },
    rejectActivation: () => { failActivation = true; },
    rejectFocus: () => { failFocus = true; },
    count: (name) => calls.filter((call) => (Array.isArray(call) ? call[0] : call) === name).length,
  };
}

test('independent Ability windows share one init and outlive the initial window', async () => {
  const h = harness();
  const initial = h.ability();
  await h.stage(initial);
  const peer = h.peer();
  await h.stage(peer);
  assert.equal(h.count('init'), 1);
  assert.equal(h.count('stage-create'), 1);
  assert.deepEqual(h.calls.slice(0, 7), ['prepare', ['init', 'session-1:rssh_lib'], 'configure', 'attach-lifecycle', 'attach-sinks', 'ability-create', 'activate']);
  await initial.onDestroy();
  assert.equal(h.windows[0].attached.has(1), true);
  assert.equal(h.count('dispose-bridge'), 0);
  assert.equal(h.count('ability-destroy'), 0);
  await peer.onDestroy();
  assert.equal(h.count('dispose-bridge'), 1);
  assert.equal(h.count('dispose-host'), 1);
  assert.equal(h.count('ability-destroy'), 1);
  assert.equal(h.count('stage-destroy'), 1);
  assert.deepEqual(h.errors, []);
});

test('a second Entry focuses surviving peers instead of reviving logical window zero', async () => {
  const h = harness();
  const initial = h.ability();
  await h.stage(initial);
  const peer = h.peer();
  await h.stage(peer);
  await initial.onDestroy();
  const replacement = h.ability();
  await h.stage(replacement);
  await replacement.onDestroy();
  assert.equal(h.count('init'), 1);
  assert.equal(h.count('attach'), 2);
  assert.equal(h.count('focus-existing'), 1);
  assert.equal(h.count('terminate'), 1);
  assert.equal(h.windows[0].attached.has(1), true);
  await peer.onDestroy();
});

test('concurrent Entry creation publishes the initialization barrier before importing', async () => {
  const h = harness();
  const load = gate();
  h.blockLoad(load.promise);
  const first = h.ability();
  const second = h.ability();
  await nextTurn();
  assert.equal(h.count('prepare'), 1);
  load.resolve();
  await h.stage(first);
  await h.stage(second);
  assert.equal(h.count('init'), 1);
  assert.equal(h.count('attach'), 1);
  await second.onDestroy();
  assert.equal(h.count('dispose-bridge'), 0);
  await first.onDestroy();
});

test('destroy during initial import waits and releases the application without mounting', async () => {
  const h = harness();
  const load = gate();
  h.blockLoad(load.promise);
  const initial = h.ability();
  initial.onWindowStageCreate({});
  const destroyed = initial.onDestroy();
  await nextTurn();
  assert.equal(h.count('init'), 0);
  load.resolve();
  await destroyed;
  assert.equal(h.count('init'), 1);
  assert.equal(h.count('attach'), 0);
  assert.equal(h.count('dispose-bridge'), 1);
  assert.equal(h.count('dispose-host'), 1);
});

test('stage attach failure terminates its Ability and tears down the empty application', async () => {
  const h = harness();
  h.rejectAttach();
  const initial = h.ability();
  await h.stage(initial);
  await initial.onDestroy();
  assert.equal(h.count('terminate'), 1);
  assert.equal(h.count('dispose-bridge'), 1);
  assert.equal(h.count('dispose-host'), 1);
  assert.match(h.errors.join('\n'), /window attach failed/);
});

test('initialization failure rolls back the bridge and never retries native init in-process', async () => {
  const h = harness();
  h.rejectActivation();
  const initial = h.ability();
  await h.stage(initial);
  await initial.onDestroy();
  const replacement = h.ability();
  await h.stage(replacement);
  await replacement.onDestroy();
  assert.equal(h.count('init'), 1);
  assert.equal(h.count('dispose-bridge'), 1);
  assert.equal(h.count('dispose-host'), 1);
  assert.equal(h.count('terminate'), 2);
  assert.match(h.errors.join('\n'), /plugin installation failed/);
});

test('unknown or malformed peer identities cannot create a fresh native application', async () => {
  const h = harness();
  const unknown = h.peer();
  await h.stage(unknown);
  await unknown.onDestroy();
  const malformed = h.ability({ nativeApplicationSession: 'session-1', nativeWindowId: 0, nativeWindowNonce: 'bad' });
  await h.stage(malformed);
  await malformed.onDestroy();
  assert.equal(h.count('init'), 0);
  assert.equal(h.count('terminate'), 2);
  assert.match(h.errors.join('\n'), /session is unavailable/);
  assert.match(h.errors.join('\n'), /Invalid native application window identity/);
});

for (const action of ['onDestroy', 'onWindowStageDestroy']) {
  test(`${action} detaches before waiting for an unfinished surface mount`, async () => {
    const h = harness();
    h.blockMount();
    const initial = h.ability();
    initial.onWindowStageCreate({});
    await nextTurn();
    assert.equal(h.count('attach'), 1);
    const destroyed = initial[action]();
    await nextTurn();
    assert.equal(h.count('detach'), 1, 'destroy must release the surface that stageWork is waiting for');
    await destroyed;
    await initial.stageWork;
    assert.equal(h.count('dispose-bridge'), 1);
    assert.equal(h.count('dispose-host'), 1);
  });
}


test('a failed Entry reactivation reports the SDK failure without tearing down surviving windows', async () => {
  const h = harness();
  const initial = h.ability();
  await h.stage(initial);
  const peer = h.peer();
  await h.stage(peer);
  await initial.onDestroy();
  h.rejectFocus();
  const replacement = h.ability();
  await h.stage(replacement);
  await replacement.onDestroy();
  assert.match(h.errors.join('\n'), /application-create.*SDK refused to activate existing windows/);
  assert.equal(h.count('init'), 1);
  assert.equal(h.count('attach'), 2);
  assert.equal(h.count('dispose-bridge'), 0);
  assert.equal(h.windows[0].attached.has(1), true);
  await peer.onDestroy();
});
