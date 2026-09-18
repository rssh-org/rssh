import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';
import { setImmediate as nextTurn } from 'node:timers/promises';
import test from 'node:test';
import ts from 'typescript';

// Run after prepare-ohos: execute the actual pinned framework, with only SDK
// objects and the loaded native module replaced. No device or app data is used.
const framework = fileURLToPath(new URL('../../target/ohos-sources/ability/native_ability/src/main/ets/', import.meta.url));
function gate() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

function harness() {
  const cache = new Map();
  const errors = [];
  const initCalls = [];
  const disposeCalls = [];
  const factories = [];
  let currentOwner;
  let nextLoad;
  const sdk = {
    '@kit.AbilityKit': { UIAbility: class {} },
    '@ohos.arkui.node': { FrameNode: class {} },
    '@ohos.window': { default: {} },
  };
  const nativeModule = {
    init(_bindings, owner, context) {
      if (currentOwner) throw new Error('native module still owned');
      currentOwner = owner;
      initCalls.push({ owner, context });
      return {
        bridgePlugins: [{ id: 'test.slow', execution: 'async', requires: ['ability'] }],
        windowStageEventCallback: {
          onAbilityCreate() {}, onAbilityDestroy() {}, onWindowStageCreate() {}, onWindowStageDestroy() {},
        },
      };
    },
    disposeBridge(owner) {
      disposeCalls.push(owner);
      if (currentOwner === owner) currentOwner = undefined;
    },
    disposeAllRenders() {},
    onBridgeLifecycle() {},
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
        if (id.endsWith('/components/MainPage')) return { RouteName: 'NativeAbility' };
        if (id.endsWith('/runtime/NativeModuleLoader')) return {
          NativeModuleLoader: {
            resolveModuleNames: (name) => [name],
            async load() { if (nextLoad) { const pending = nextLoad; nextLoad = undefined; await pending; } return nativeModule; },
          },
        };
        if (id.startsWith('.')) return load(path.relative(framework, path.resolve(path.dirname(filename), id + '.ets')));
        throw new Error(`Unexpected native dependency: ${id}`);
      },
      console: { error: (...args) => errors.push(args.join(' ')) },
      AppStorage: { setOrCreate() {}, set() {} },
      setTimeout, clearTimeout,
    }, { filename });
    return exports;
  }
  const { NativeAbility } = load('ability/NativeAbility.ets');
  const { BridgeHostRegistry } = load('bridge/BridgeHost.ets');
  function ability(disposeGate) {
    const instance = new NativeAbility();
    instance.moduleName = 'rssh_lib';
    instance.defaultPage = false;
    instance.context = { filesDir: `/isolated/session-${factories.length}`, config: { language: 'zh' } };
    const disposed = gate();
    instance.bridgePlugins = [{ create() {
      const plugin = {
        id: 'test.slow', execution: 'async', requires: ['ability'],
        onInstall() {}, async invokeAsync() { throw new Error('unused'); },
        async onDispose() { disposed.resolve(); if (disposeGate) await disposeGate; },
      };
      factories.push(plugin);
      return plugin;
    } }];
    return { instance, disposed };
  }
  const drain = (instance) => instance.enqueueLifecycleOperation('test barrier', async () => {});
  const create = (instance) => instance.onCreate({ parameters: {} }, {});
  return {
    ability, drain, create, errors, initCalls, disposeCalls, BridgeHostRegistry,
    currentOwner: () => currentOwner,
    delayNextLoad: (promise) => { nextLoad = promise; },
  };
}

test('a new Ability waits for an old closing session and refreshes its native context', async () => {
  const h = harness();
  const release = gate();
  const first = h.ability(release.promise);
  h.create(first.instance);
  await h.drain(first.instance);
  const firstOwner = h.currentOwner();
  const destroying = first.instance.onDestroy();
  await first.disposed.promise;
  const second = h.ability();
  try {
    h.create(second.instance);
    await nextTurn();
    assert.equal(h.initCalls.length, 1, 'new init must not run during old plugin disposal');
    release.resolve();
    await Promise.all([destroying, h.drain(second.instance)]);
    assert.equal(h.initCalls.length, 2, h.errors.join('\n'));
    assert.notEqual(h.initCalls[1].owner, firstOwner);
    assert.equal(h.initCalls[1].context.basePath, second.instance.context.filesDir);
    await h.BridgeHostRegistry.dispose(firstOwner.split(':')[0]);
    assert.equal(h.currentOwner(), h.initCalls[1].owner, 'stale release cannot clear new native owner');
  } finally {
    release.resolve();
    await destroying;
    await second.instance.onDestroy();
  }
});

test('a genuinely active owner still rejects a second concurrent Ability', async () => {
  const h = harness();
  const first = h.ability();
  const second = h.ability();
  h.create(first.instance);
  await h.drain(first.instance);
  try {
    h.create(second.instance);
    await h.drain(second.instance);
    assert.equal(h.initCalls.length, 1);
    assert.match(h.errors.join('\n'), /already belongs to active Ability session/);
    assert.equal(h.currentOwner(), h.initCalls[0].owner);
  } finally {
    await second.instance.onDestroy();
    await first.instance.onDestroy();
  }
});

test('destroying a queued replacement does not initialize or retain its module', async () => {
  const h = harness();
  const release = gate();
  const first = h.ability(release.promise);
  h.create(first.instance);
  await h.drain(first.instance);
  const destroying = first.instance.onDestroy();
  await first.disposed.promise;
  const second = h.ability();
  h.create(second.instance);
  await nextTurn();
  const cancelReplacement = second.instance.onDestroy();
  release.resolve();
  await Promise.all([destroying, cancelReplacement]);
  assert.equal(h.initCalls.length, 1);
  assert.equal(h.currentOwner(), undefined);
  const third = h.ability();
  h.create(third.instance);
  await h.drain(third.instance);
  assert.equal(h.initCalls.length, 2);
  await third.instance.onDestroy();
});

test('destroy during native loading releases ownership before a replacement starts', async () => {
  const h = harness();
  const loading = gate();
  h.delayNextLoad(loading.promise);
  const first = h.ability();
  h.create(first.instance);
  await nextTurn();
  const destroying = first.instance.onDestroy();
  const second = h.ability();
  h.create(second.instance);
  await nextTurn();
  loading.resolve();
  await Promise.all([destroying, h.drain(second.instance)]);
  try {
    assert.equal(h.initCalls.length, 1, h.errors.join('\n'));
    assert.equal(h.initCalls[0].context.basePath, second.instance.context.filesDir);
  } finally {
    await second.instance.onDestroy();
  }
});
