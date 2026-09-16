import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';

const ability = fileURLToPath(new URL('../../target/ohos-sources/ability/', import.meta.url));
const ets = path.join(ability, 'native_ability/src/main/ets');

async function runtime(useSerial = false) {
  let now = 0;
  let nextTimer = 0;
  const timers = new Map();
  const globals = {
    console,
    canIUse: () => true,
    setTimeout(callback, delay) { const id = ++nextTimer; timers.set(id, { callback, at: now + delay }); return id; },
    clearTimeout(id) { timers.delete(id); },
  };
  function compile(file, dependencies = {}, suffix = '') {
    const source = readFileSync(path.isAbsolute(file) ? file : path.join(ets, file), 'utf8') + suffix;
    const output = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
    const exports = {};
    vm.runInNewContext(output, { ...globals, exports, require: (name) => dependencies[name] });
    return exports;
  }
  const { BridgeHost } = compile('bridge/BridgeHost.ets', {
    '../runtime/SerialTaskQueue': compile('runtime/SerialTaskQueue.ets'),
    '../runtime/CancellableTaskScope': compile('runtime/CancellableTaskScope.ets'),
  }, '\nexport { BridgeHost };');
  let complete;
  let entered;
  let context;
  let cancellations = 0;
  const started = new Promise((resolve) => { entered = resolve; });
  const selection = new Promise((resolve) => { complete = resolve; });
  let plugin = {
    id: 'ohos.files', execution: 'async', requires: ['ability'],
    async invokeAsync(_action, _request, callContext) {
      context = callContext;
      context.onCancel(() => { cancellations++; });
      entered();
      return { typeName: 'ohos.files.DialogResponse', value: await selection };
    },
  };
  let closed = 0;
  let listeners = 0;
  if (useSerial) {
    const port = {
      portInfo: { portName: '/dev/ttyACM0' },
      async open() { entered(); await selection; },
      async close() { closed++; },
      onDataRead() { listeners++; },
      onDisconnect() { listeners++; },
    };
    const source = fileURLToPath(new URL('../../gen/ohos/entry/src/main/ets/plugins/SerialPlugin.ets', import.meta.url));
    const { SerialPlugin } = compile(source, {
      '@ohos.deviceInfo': { default: { sdkApiVersion: 26 } },
      '@ohos.busManager.serial': { default: { getSerialPortList: async () => [port] } },
      '@ohos-rs/ability': { AsyncPluginBase: class {
        execution = 'async';
        attachContext(value) { this.context = value; }
        getContext() { return this.context; }
      } },
    });
    plugin = new SerialPlugin();
    const invoke = plugin.invokeAsync.bind(plugin);
    plugin.invokeAsync = (action, payload, callContext) => {
      if (action === 'open') {
        context = callContext;
        context.onCancel(() => { cancellations++; });
      }
      return invoke(action, payload, callContext);
    };
  }
  const host = new BridgeHost('picker-session', 'test_native', {}, [{ create: () => plugin }]);
  host.configurePlugins([{ id: plugin.id, execution: plugin.execution, requires: plugin.requires }]);
  host.attachEventSink(() => ({}), () => {});
  await host.activateAbility({ kind: 'ability-create', payload: {} });
  return {
    host, started, complete,
    context: () => context,
    cancellations: () => cancellations,
    closed: () => closed,
    listeners: () => listeners,
    invoke: (timeout) => useSerial
      ? host.invokeAsync(plugin.id, 'open', 'rssh.serial.OpenRequest', 'rssh.serial.EmptyResponse', {
        id: 'attempt-1', port: '/dev/ttyACM0', baudRate: 115200, dataBits: 8,
        parity: 'none', stopBits: 1, flowControl: 'none', xany: false,
      }, timeout)
      : host.invokeAsync(plugin.id, 'file-dialog', 'ohos.files.DialogOptions', 'ohos.files.DialogResponse', {}, timeout),
    closeSerial: () => host.invokeAsync(plugin.id, 'close', 'rssh.serial.PortRequest', 'rssh.serial.EmptyResponse', { id: 'attempt-1' }, 15_000),
    async advance(milliseconds) {
      now += milliseconds;
      for (const [id, timer] of Array.from(timers)) {
        if (timer.at <= now) { timers.delete(id); timer.callback(); }
      }
      for (let i = 0; i < 10; i++) await Promise.resolve();
    },
  };
}

for (const selected of [['file://docs/selected.txt'], []]) {
  test(`human picker remains pending past one minute and returns ${selected.length ? 'a selection' : 'user cancellation'}`, async () => {
    const r = await runtime();
    const pending = r.invoke(0);
    let settled = false;
    pending.then(() => { settled = true; }, () => { settled = true; });
    await r.started;
    await r.advance(120_000);
    assert.equal(settled, false);
    assert.equal(r.context().isCancelled(), false);
    r.complete(selected);
    assert.deepEqual(await pending, selected);
    await r.host.dispose();
  });
}

test('no-deadline picker is still cancelled by Ability teardown and cannot return a late selection', async () => {
  const r = await runtime();
  const pending = r.invoke(0);
  const rejected = assert.rejects(pending, /closing/);
  await r.started;
  await r.advance(120_000);
  r.host.beginClosing();
  await rejected;
  assert.equal(r.context().isCancelled(), true);
  assert.equal(r.cancellations(), 1);
  r.complete(['file://docs/late.txt']);
  await r.host.dispose();
});

test('serial system authorization can outlast one minute and open successfully', async () => {
  const r = await runtime(true);
  const pending = r.invoke(0);
  let settled = false;
  pending.then(() => { settled = true; }, () => { settled = true; });
  await r.started;
  await r.advance(120_000);
  assert.equal(settled, false);
  assert.equal(r.context().isCancelled(), false);
  r.complete();
  await pending;
  assert.equal(r.listeners(), 2);
  await r.closeSerial();
  assert.equal(r.closed(), 1);
  await r.host.dispose();
});

for (const close of ['native-close', 'ability-close']) {
  test(`serial late authorization after 120 seconds and ${close} is rejected and releases the port`, async () => {
    const r = await runtime(true);
    const pending = r.invoke(0);
    const rejected = assert.rejects(pending, /cancelled|closing/);
    await r.started;
    await r.advance(120_000);
    if (close === 'native-close') await r.closeSerial();
    else r.host.beginClosing();
    r.complete();
    await rejected;
    // The bridge can reject on cancellation before the system authorization
    // resolves. Let the native plugin finish its late-result cleanup as well.
    await r.advance(0);
    assert.equal(r.closed(), 1);
    assert.equal(r.listeners(), 0);
    await r.host.dispose();
  });
}

test('ordinary bridge operations retain their bounded deadline and cancellation signal', async () => {
  const r = await runtime();
  const pending = r.invoke(100);
  const rejected = assert.rejects(pending, /timed out after 100ms/);
  await r.started;
  await r.advance(99);
  assert.equal(r.context().isCancelled(), false);
  await r.advance(1);
  await rejected;
  assert.equal(r.context().isCancelled(), true);
  assert.equal(r.cancellations(), 1);
  r.complete([]);
  await r.host.dispose();
});

test('Rust human authorization calls opt into no deadline while bounded policy stays compatible', () => {
  const source = readFileSync(path.join(ability, 'crates/ability/src/bridge/mod.rs'), 'utf8');
  const options = source.slice(source.indexOf('/// Per-call policy'), source.indexOf('\nstruct BridgeRequest'));
  const constants = source.match(/const (?:DEFAULT|MAX)_TIMEOUT_MS: u32 = [^;]+;/g).join('\n');
  const directory = mkdtempSync(path.join(tmpdir(), 'rssh-bridge-deadline-'));
  try {
    const file = path.join(directory, 'policy.rs');
    writeFileSync(file, `${constants}\n${options}\nfn main() {
      assert_eq!(BridgeCallOptions::default().timeout_ms(), 15_000);
      assert_eq!(BridgeCallOptions::default().with_timeout_ms(0).timeout_ms(), 1);
      assert_eq!(BridgeCallOptions::default().with_timeout_ms(90_000).timeout_ms(), 60_000);
      assert_eq!(BridgeCallOptions::default().without_timeout().timeout_ms(), 0);
    }`);
    execFileSync('rustc', ['--edition=2021', file, '-o', path.join(directory, 'policy')]);
    execFileSync(path.join(directory, 'policy'));
  } finally { rmSync(directory, { recursive: true, force: true }); }
  const facade = readFileSync(path.join(ability, 'crates/plugin-files/src/lib.rs'), 'utf8');
  assert.match(facade, /"file-dialog",\s*options,\s*BridgeCallOptions::default\(\)\.without_timeout\(\)/);
  const serial = readFileSync(new URL('../../src/ohos/serial.rs', import.meta.url), 'utf8');
  assert.match(serial.slice(serial.indexOf('pub async fn open(')), /BridgeCallOptions::default\(\)\.without_timeout\(\)/);
});
