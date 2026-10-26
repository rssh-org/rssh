import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { prepareProject } from './prepare-project.mjs';

test('OHOS staging synchronizes native sources without importing IDE or build state', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh ohos project '));
  const source = path.join(root, 'ohos/app');
  const generated = path.join(root, 'src-tauri/gen/ohos');
  const write = (base, file, value) => {
    const target = path.join(base, file);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, value);
  };
  try {
    write(source, 'AppScope/app.json5', '{app:{versionName:"source"}}');
    write(source, 'common/runtime/src/main/ets/plugin.ets', 'source');
    write(source, 'products/phone/src/main/module.json5', '{}');
    write(source, 'common/runtime/build/stale', 'build output');
    write(source, 'common/runtime/libs/arm64-v8a/old.so', 'library');
    write(source, 'common/runtime/src/main/resources/rawfile/old.html', 'frontend');
    write(source, 'vendor/ability/old.ets', 'generated dependency');
    write(source, 'oh_modules/old', 'dependency');
    write(source, '.idea/workspace.xml', 'IDE state');
    write(source, 'signing.local.json', '{"signingConfigs":[]}');
    write(source, 'signing/test.p12', 'test-only signing fixture');
    write(source, 'local.properties', 'sdk.dir=/test/sdk');
    write(generated, 'products/phone/build/cache', 'keep cache');
    prepareProject(root);
    assert.equal(readFileSync(path.join(generated, 'common/runtime/src/main/ets/plugin.ets'), 'utf8'), 'source');
    assert.equal(readFileSync(path.join(generated, 'products/phone/build/cache'), 'utf8'), 'keep cache');
    for (const file of ['common/runtime/build/stale', 'common/runtime/libs/arm64-v8a/old.so', 'common/runtime/src/main/resources/rawfile/old.html', 'vendor/ability/old.ets', 'oh_modules/old', '.idea/workspace.xml']) {
      assert.equal(existsSync(path.join(generated, file)), false, file);
    }
    assert.equal(readFileSync(path.join(generated, 'signing/test.p12'), 'utf8'), 'test-only signing fixture');
    assert.equal(readFileSync(path.join(generated, 'local.properties'), 'utf8'), 'sdk.dir=/test/sdk');
    const tracked = JSON.parse(readFileSync(path.join(generated, '.rssh-source-files.json')));
    assert.ok(tracked.every((file) => !file.startsWith('signing/') && file !== 'signing.local.json' && file !== 'local.properties'));
    rmSync(path.join(source, 'common/runtime/src/main/ets/plugin.ets'));
    rmSync(path.join(source, 'signing.local.json'));
    rmSync(path.join(source, 'signing'), { recursive: true });
    write(source, 'AppScope/app.json5', '{app:{versionName:"changed"}}');
    prepareProject(root);
    assert.equal(existsSync(path.join(generated, 'common/runtime/src/main/ets/plugin.ets')), false);
    assert.equal(existsSync(path.join(generated, 'signing.local.json')), false);
    assert.equal(existsSync(path.join(generated, 'signing')), false);
    assert.equal(readFileSync(path.join(generated, 'AppScope/app.json5'), 'utf8'), '{app:{versionName:"changed"}}');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
