import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';

const filename = fileURLToPath(new URL(
  '../../../target/ohos-sources/ability/plugins/files/src/main/ets/FilesPlugin.ets', import.meta.url,
));
const code = ts.transpileModule(readFileSync(filename, 'utf8'), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;
const folderCapability = 'SystemCapability.FileManagement.UserFileService.FolderSelection';

function harness(deviceType, folderSelection) {
  const selections = [];
  const picker = {
    DocumentSelectOptions: class {},
    DocumentSelectMode: { FILE: 0, FOLDER: 1 },
    DocumentViewPicker: class {
      async select(options) {
        selections.push(options);
        return ['file://docs/selected'];
      }
    },
  };
  const dependencies = {
    '@ohos.deviceInfo': { default: { deviceType, sdkApiVersion: 25 } },
    '@ohos.file.picker': { default: picker },
    '@ohos-rs/ability': { AsyncPluginBase: class {} },
  };
  const exports = {};
  vm.runInNewContext(code, {
    exports,
    canIUse: (capability) => capability === folderCapability && folderSelection,
    require: (name) => {
      if (!(name in dependencies)) throw new Error(`Unexpected native dependency: ${name}`);
      return dependencies[name];
    },
  }, { filename });
  const plugin = new exports.FilesPlugin();
  const select = (dialogType) => plugin.invokeAsync('file-dialog', {
    typeName: 'ohos.files.DialogOptions', value: { dialogType, allowMany: false, filters: [] },
  }, { getAbilityContext: () => ({}), isActive: () => true });
  return { select, selections };
}

for (const deviceType of ['phone', 'tablet', '2in1']) {
  test(`${deviceType} folder selection uses the available system capability`, async () => {
    const h = harness(deviceType, true);
    const result = await h.select('open-folder');
    assert.equal(result.value.files[0], 'file://docs/selected');
    assert.equal(h.selections[0].selectMode, 1);
    assert.equal(h.selections[0].maxSelectNumber, 1);
  });

  test(`${deviceType} without folder capability still permits single-file selection`, async () => {
    const h = harness(deviceType, false);
    await assert.rejects(h.select('open-folder'), /FolderSelection capability/);
    assert.equal(h.selections.length, 0);
    const result = await h.select('open-file');
    assert.equal(result.value.files[0], 'file://docs/selected');
    assert.equal(h.selections[0].selectMode, undefined);
  });
}
