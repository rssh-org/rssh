import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { setImmediate as nextTurn } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';

const ability = fileURLToPath(new URL('../../target/ohos-sources/ability/', import.meta.url));
const ets = path.join(ability, 'native_ability/src/main/ets');

async function runtime(useSerial = false, serialApi = 26) {
  let now = 0;
  let nextTimer = 0;
  const timers = new Map();
  const globals = {
    console,
    canIUse: (capability) => capability === (serialApi >= 26
      ? 'SystemCapability.BusManager.Serial' : 'SystemCapability.USB.USBManager.Serial'),
    setTimeout(callback, delay) { const id = ++nextTimer; timers.set(id, { callback, at: now + delay }); return id; },
    clearTimeout(id) { timers.delete(id); },
  };
  function compile(file, dependencies = {}, suffix = '') {
    const filename = path.isAbsolute(file) ? file : path.join(ets, file);
    const source = readFileSync(filename, 'utf8') + suffix;
    const output = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
    const exports = {};
    vm.runInNewContext(output, { ...globals, exports, require: (name) => {
      if (Object.hasOwn(dependencies, name)) return dependencies[name];
      if (name.startsWith('.')) return compile(path.resolve(path.dirname(filename), `${name}.ets`), dependencies);
      throw new Error(`Unexpected native dependency: ${name}`);
    } }, { filename });
    return exports;
  }
  const { BridgeHost } = compile('bridge/BridgeHost.ets', {
    '@ohos.arkui.node': {},
    '@ohos.window': { default: {} },
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
  let opened = 0;
  let configured;
  let finishRead;
  let readEntered;
  let readCompletions = 0;
  const reading = new Promise((resolve) => { readEntered = resolve; });
  let closeEntered;
  const nativeClosing = new Promise((resolve) => { closeEntered = resolve; });
  let closeGate;
  let releaseClose;
  const writeSizes = [];
  const writeChunks = [];
  const acceptedBytes = [];
  if (useSerial) {
    const port = {
      portInfo: { portName: '/dev/ttyACM0' },
      async open() { entered(); await selection; opened++; },
      async close() { closed++; closeEntered(); if (closeGate) await closeGate; },
      onDataRead() { listeners++; },
      onDisconnect() { listeners++; },
    };
    const nativeModules = serialApi >= 26 ? {
      '@ohos.busManager.serial': { default: { getSerialPortList: async () => [port] } },
    } : {
      '@ohos.usbManager.serial': { default: {
        getPortList: () => [{ portId: 7, deviceName: '/dev/ttyACM0' }],
        hasSerialRight: () => false,
        async requestSerialRight() { entered(); return await selection; },
        open() { opened++; },
        close() { closed++; },
        setAttribute(_id, attribute) { configured = attribute; },
        read(_id, _buffer, timeout) {
          assert.equal(timeout, 250);
          return new Promise((resolve) => {
            finishRead = (length) => { readCompletions++; resolve(length); };
            readEntered();
          });
        },
        async write(_id, bytes, timeout) {
          assert.equal(timeout, 1000);
          assert.ok(bytes.length <= 4096);
          writeChunks.push(Array.from(bytes));
          const length = Math.min(writeSizes.shift() ?? bytes.length, bytes.length);
          acceptedBytes.push(...Array.from(bytes.slice(0, length)));
          return length;
        },
      } },
    };
    const source = fileURLToPath(new URL('../../gen/ohos/common/runtime/src/main/ets/plugins/SerialPlugin.ets', import.meta.url));
    const { SerialPlugin } = compile(source, {
      ...nativeModules,
      '@ohos.deviceInfo': { default: { sdkApiVersion: serialApi } },
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
    host, started, complete, reading, nativeClosing,
    context: () => context,
    cancellations: () => cancellations,
    closed: () => closed,
    opened: () => opened,
    configured: () => configured,
    readCompletions: () => readCompletions,
    completeRead: (length = 0) => { assert.ok(finishRead, 'a native read must be pending'); finishRead(length); },
    holdNativeClose: () => { closeGate = new Promise((resolve) => { releaseClose = resolve; }); },
    completeNativeClose: () => { assert.ok(releaseClose, 'a native close must be held'); releaseClose(); },
    writeSizes: (...sizes) => writeSizes.push(...sizes),
    writeChunks: () => writeChunks,
    acceptedBytes: () => acceptedBytes,
    listeners: () => listeners,
    invoke: (timeout, settings = {}) => useSerial
      ? host.invokeAsync(plugin.id, 'open', 'rssh.serial.OpenRequest', 'rssh.serial.EmptyResponse', {
        id: 'attempt-1', port: '/dev/ttyACM0', baudRate: 115200, dataBits: 8,
        parity: 'none', stopBits: 1, flowControl: 'none', xany: false,
        ...settings,
      }, timeout)
      : host.invokeAsync(plugin.id, 'file-dialog', 'ohos.files.DialogOptions', 'ohos.files.DialogResponse', {}, timeout),
    closeSerial: (id = 'attempt-1') => host.invokeAsync(plugin.id, 'close', 'rssh.serial.PortRequest', 'rssh.serial.EmptyResponse', { id }, 0),
    writeSerial: (data) => host.invokeAsync(plugin.id, 'write', 'rssh.serial.WriteRequest', 'rssh.serial.EmptyResponse', { id: 'attempt-1', data }, 15_000),
    serialCapabilities: () => host.invokeAsync(plugin.id, 'capabilities', 'rssh.serial.EmptyRequest', 'rssh.serial.CapabilitiesResponse', {}, 15_000),
    async advance(milliseconds) {
      now += milliseconds;
      for (const [id, timer] of Array.from(timers)) {
        if (timer.at <= now) { timers.delete(id); timer.callback(); }
      }
      // Drain the current microtask queue without advancing the native mocks'
      // clock. A fixed number of Promise ticks depends on bridge await depth.
      await nextTurn();
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
    let closing;
    let closeSettled = false;
    if (close === 'native-close') {
      r.holdNativeClose();
      closing = r.closeSerial();
      closing.then(() => { closeSettled = true; }, () => { closeSettled = true; });
      await r.advance(120_000);
      assert.equal(closeSettled, false, 'close must wait while authorization is pending');
    } else r.host.beginClosing();
    r.complete();
    if (closing) {
      await r.nativeClosing;
      await r.advance(0);
      assert.equal(r.opened(), 1);
      assert.equal(r.listeners(), 0);
      assert.equal(closeSettled, false, 'late-open cleanup must finish before close resolves');
      r.completeNativeClose();
      await closing;
    }
    await rejected;
    // The bridge can reject on cancellation before the system authorization
    // resolves. Let the native plugin finish its late-result cleanup as well.
    await r.advance(0);
    assert.equal(r.closed(), 1);
    assert.equal(r.listeners(), 0);
    await r.host.dispose();
  });
}

for (const close of ['native-close', 'ability-close']) {
  test(`API 19 authorization cancelled by ${close} never opens the port after a late grant`, async () => {
    const r = await runtime(true, 19);
    const pending = r.invoke(0);
    const rejected = assert.rejects(pending, /cancelled|closing/);
    await r.started;
    await r.advance(120_000);
    assert.equal(r.context().isCancelled(), false);
    let closing;
    let closeSettled = false;
    if (close === 'native-close') {
      closing = r.closeSerial();
      closing.then(() => { closeSettled = true; }, () => { closeSettled = true; });
      await r.advance(120_000);
      assert.equal(closeSettled, false, 'close must wait while authorization is pending');
    } else r.host.beginClosing();
    r.complete(true);
    if (closing) await closing;
    await rejected;
    await r.advance(0);
    assert.equal(r.opened(), 0);
    assert.equal(r.closed(), 0);
    assert.equal(r.readCompletions(), 0);
    await r.host.dispose();
  });
}

for (const api of [19, 26]) {
  test(`API ${api} Ability disposal finishes before authorization and cleans up a late grant`, async () => {
    const r = await runtime(true, api);
    const pending = r.invoke(0);
    const rejected = assert.rejects(pending, /cancelled|closing/);
    await r.started;
    await r.host.dispose();
    await rejected;
    assert.equal(r.opened(), 0);
    assert.equal(r.closed(), 0);

    r.complete(true);
    await r.advance(0);
    assert.equal(r.opened(), api >= 26 ? 1 : 0);
    assert.equal(r.closed(), api >= 26 ? 1 : 0);
    assert.equal(r.listeners(), 0);
  });
}

test('API 19 long authorization preserves stop-bit encoding and short writes without dropping or repeating bytes', async () => {
  const r = await runtime(true, 19);
  const capabilities = await r.serialCapabilities();
  assert.equal(capabilities.flowControl, false);
  assert.equal(capabilities.xany, false);
  assert.equal(capabilities.signals, false);
  assert.equal(capabilities.baudRates.includes(115200), true);
  assert.equal(capabilities.baudRates.includes(4000000), true);
  assert.equal(capabilities.baudRates.every((rate) => rate > 0), true);
  const pending = r.invoke(0, { stopBits: 2 });
  let settled = false;
  pending.then(() => { settled = true; }, () => { settled = true; });
  await r.started;
  await r.advance(120_000);
  assert.equal(settled, false);
  r.complete(true);
  await pending;
  await r.reading;
  assert.equal(r.configured().stopBits, 1);
  const bytes = Array.from({ length: 5000 }, (_, index) => index % 256);
  r.writeSizes(3, 1024);
  await r.writeSerial(bytes);
  assert.deepEqual(r.acceptedBytes(), bytes);
  assert.equal(r.writeChunks().length, 3);
  assert.equal(r.writeChunks().every((chunk) => chunk.length <= 4096), true);
  const closing = r.closeSerial();
  await r.advance(0);
  r.completeRead();
  await closing;
  assert.equal(r.closed(), 1);
  await r.host.dispose();
});

test('API 19 close keeps the device reserved until the old asynchronous read finishes', async () => {
  const r = await runtime(true, 19);
  const pending = r.invoke(0);
  await r.started;
  r.complete(true);
  await pending;
  await r.reading;
  const closing = r.closeSerial();
  let closed = false;
  closing.then(() => { closed = true; }, () => { closed = true; });
  await r.advance(0);
  assert.equal(closed, false);
  assert.equal(r.closed(), 0);
  await assert.rejects(r.invoke(0, { id: 'attempt-2' }), /already in use/);
  r.completeRead();
  await closing;
  assert.equal(r.readCompletions(), 1);
  assert.equal(r.closed(), 1);

  // A later attempt can now own the numeric native port. Closing the old
  // attempt must not close that new owner or consume its pending read.
  await r.invoke(0, { id: 'attempt-2' });
  assert.equal(r.opened(), 2);
  await r.closeSerial('attempt-1');
  assert.equal(r.closed(), 1);
  const closingAgain = r.closeSerial('attempt-2');
  await r.advance(0);
  r.completeRead();
  await closingAgain;
  assert.equal(r.closed(), 2);
  await r.host.dispose();
});

test('API 19 rejects unsupported flow control before asking for authorization', async () => {
  const r = await runtime(true, 19);
  await assert.rejects(r.invoke(0, { flowControl: 'hardware' }), /does not support flow control/);
  assert.equal(r.opened(), 0);
  await r.host.dispose();
});

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
  const serialClose = serial.slice(serial.indexOf('pub async fn close('), serial.indexOf('pub async fn write('));
  assert.match(serialClose, /BridgeCallOptions::default\(\)\.without_timeout\(\)/);
});
