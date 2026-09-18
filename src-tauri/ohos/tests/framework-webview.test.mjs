import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';
import test from 'node:test';
import ts from 'typescript';

// Exercise the pinned ArkTS WebView plugin and its actual builder callbacks.
// The SDK rendering/controller objects are fakes; no WebView or device is started.
const sourcePath = fileURLToPath(new URL('../../target/ohos-sources/ability/plugins/webview/src/main/ets/WebviewPlugin.ets', import.meta.url));
const code = ts.transpileModule(readFileSync(sourcePath, 'utf8'), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;

function harness() {
  const callbacks = {};
  const events = [];
  const errors = [];
  let rejectNotification = false;
  const context = {
    sessionId: 'vm-session', isActive: () => true, isCancelled: () => false,
    onCancel: () => () => {}, getUIContext: () => ({}), appendChild() {}, removeChild() {},
    invokeNativeSync(event, requestTypeName, responseTypeName, value) {
      events.push({ event, requestTypeName, responseTypeName, value });
      return { accepted: !rejectNotification };
    },
    invokeNativeSyncProcessWide: () => [{ accepted: true, schemes: [] }],
  };
  const chain = new Proxy({}, { get: (_target, key) => (value) => {
    if (String(key).startsWith('on')) callbacks[key] = value;
    return chain;
  } });
  const modules = {
    '@ohos-rs/ability': { AsyncPluginBase: class { getContext() { return context; } } },
    '@ohos.arkui.node': { BuilderNode: class {
      build(builder, data) { builder(data); callbacks.onControllerAttached(); }
      getFrameNode() { return { dispose() {} }; }
    } },
    '@ohos.web.webview': { default: { WebviewController: class {
      static initializeWebEngine() {}
      loadUrl() {}
    } } },
  };
  const exports = {};
  runInNewContext(code, {
    exports,
    require(id) { if (!(id in modules)) throw new Error(`Unexpected SDK import ${id}`); return modules[id]; },
    wrapBuilder: (builder) => builder, Web: () => chain,
    Visibility: { Hidden: 0, Visible: 1 },
    console: { error: (...args) => errors.push(args.join(' ')), warn() {} },
  }, { filename: sourcePath });
  const plugin = new exports.WebviewPlugin();
  plugin.onInstall(context);
  async function create(pageLoad = true) {
    await plugin.invokeAsync('create', { typeName: 'ohos.webview.CreateRequest', value: {
      id: 'main', url: 'https://example.test', eventOptions: { pageLoad },
    } }, context);
  }
  return { callbacks, events, errors, create, reject: () => { rejectNotification = true; } };
}

test('page begin and end forward named events with the controller identity and URL', async () => {
  const h = harness();
  await h.create();
  h.callbacks.onPageBegin({ url: 'https://example.test/loading' });
  h.callbacks.onPageEnd({ url: 'https://example.test/finished' });
  const attached = h.events.find((event) => event.event === 'controller-attached').value;
  const page = h.events.filter((event) => event.event === 'page-load');
  assert.equal(page.length, 2);
  for (const event of page) {
    assert.equal(event.requestTypeName, 'ohos.webview.PageLoadEvent');
    assert.equal(event.responseTypeName, 'ohos.webview.EventAcknowledgement');
    assert.equal(event.value.id, 'main');
    assert.equal(event.value.nativeTag, attached.nativeTag);
  }
  assert.equal(page[0].value.started, true);
  assert.equal(page[0].value.url, 'https://example.test/loading');
  assert.equal(page[1].value.started, false);
  assert.equal(page[1].value.url, 'https://example.test/finished');
});

test('unsubscribed page events do not call the native bridge', async () => {
  const h = harness();
  await h.create(false);
  h.callbacks.onPageBegin({ url: 'https://example.test' });
  h.callbacks.onPageEnd({ url: 'https://example.test' });
  assert.equal(h.events.filter((event) => event.event === 'page-load').length, 0);
});

test('a rejected page notification is reported without throwing into ArkWeb', async () => {
  const h = harness();
  await h.create();
  h.reject();
  assert.doesNotThrow(() => h.callbacks.onPageEnd({ url: 'https://example.test' }));
  assert.match(h.errors.join('\n'), /page-load.*notification.*failed/i);
});
