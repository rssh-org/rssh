import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';

const file = fileURLToPath(new URL('../../../target/ohos-sources/ability/plugins/files/src/main/ets/FilesPlugin.ets', import.meta.url));

function pickerHarness(deviceType, supported) {
  const calls = [];
  const picker = {
    DocumentSelectMode: { FOLDER: 1 },
    DocumentSelectOptions: class {},
    DocumentViewPicker: class {
      async select(options) { calls.push(options); return ['file://docs/selected']; }
    },
  };
  const dependencies = {
    '@ohos.deviceInfo': { default: { deviceType, sdkApiVersion: 26 } },
    '@ohos.file.picker': { default: picker },
    '@ohos-rs/ability': { AsyncPluginBase: class {} },
  };
  const exports = {};
  vm.runInNewContext(ts.transpileModule(readFileSync(file, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText, { exports, require: name => dependencies[name], canIUse: () => supported });
  return { calls, plugin: new exports.FilesPlugin() };
}

for (const device of ['phone', 'tablet', '2in1']) {
  test(`${device} with folder capability opens the system folder picker`, async () => {
    const h = pickerHarness(device, true);
    const result = await h.plugin.invokeAsync('file-dialog', {
      typeName: 'ohos.files.DialogOptions', value: { dialogType: 'open-folder', allowMany: false, filters: [] },
    }, { getAbilityContext: () => ({}), isActive: () => true });
    assert.equal(result.value.files[0], 'file://docs/selected');
    assert.equal(h.calls.length, 1);
    assert.equal(h.calls[0].selectMode, 1);
    assert.equal(h.calls[0].maxSelectNumber, 1);
  });
}

test('a host without folder capability never substitutes a single-file picker', async () => {
  const h = pickerHarness('2in1', false);
  await assert.rejects(h.plugin.invokeAsync('file-dialog', {
    typeName: 'ohos.files.DialogOptions', value: { dialogType: 'open-folder', allowMany: false, filters: [] },
  }, { getAbilityContext: () => ({}), isActive: () => true }), /FolderSelection capability/);
  assert.equal(h.calls.length, 0);
});
