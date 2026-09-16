import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { prepareWorkspace } from './prepare-ohos.mjs';

test('OHOS workspace derives shared dependencies without changing the normal workspace', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh ohos '));
  try {
    const source = path.join(root, 'src-tauri');
    for (const dir of ['src', 'capabilities', 'icons', 'bin', 'ohos', 'gen/ohos/AppScope']) {
      mkdirSync(path.join(source, dir), { recursive: true });
    }
    const manifest = `[package]\nname = "rssh"\nversion = "0.0.1"\n[lib]\nname = "rssh_lib"\n[build-dependencies]\ntauri-build = { version = "2", features = [] }\n[dependencies]\ntauri = { version = "2", features = ["protocol-asset"] }\nnew-shared-dependency = "1"\n`;
    writeFileSync(path.join(source, 'Cargo.toml'), manifest);
    writeFileSync(path.join(source, 'Cargo.lock'), 'normal lock');
    writeFileSync(path.join(source, 'build.rs'), 'fn main() {}');
    writeFileSync(path.join(source, 'src/lib.rs'), '// shared source');
    writeFileSync(path.join(source, 'ohos/overlay.toml'), '[workspace]\n[patch.crates-io]\nwry = { path = "@OHOS_SOURCES@/wry" }\n');
    const lock = 'version = 4\n\n[[package]]\nname = "rssh"\nversion = "0.0.1"\n';
    writeFileSync(path.join(source, 'ohos/Cargo.lock'), lock);
    writeFileSync(path.join(source, 'tauri.conf.json'), JSON.stringify({
      version: '0.0.1',
      build: { frontendDist: '../dist' }, bundle: { resources: { 'bin/*': 'bin/' } },
    }));
    const appConfig = `{
      // DevEco projects use JSON5, including comments and trailing commas.
      app: { versionName: '1.0.0', versionCode: 7, bundleName: 'com.rssh.app', },
    }\n`;
    writeFileSync(path.join(source, 'gen/ohos/AppScope/app.json5'), appConfig);

    const workspace = path.join(source, 'target/ohos-workspace');
    prepareWorkspace(root, workspace);
    const generated = readFileSync(path.join(workspace, 'Cargo.toml'), 'utf8');
    assert.match(generated, /tauri-build = \{ version = "=2\.6\.3"/);
    assert.match(generated, /tauri = \{ version = "=2\.11\.5"/);
    assert.match(generated, /new-shared-dependency = "1"/);
    assert.match(generated, /\[workspace\]/);
    assert.ok(generated.includes(`wry = { path = ${JSON.stringify(path.join(source, 'target/ohos-sources/wry'))} }`));
    assert.doesNotMatch(generated, /@OHOS_SOURCES@/);
    assert.equal(readFileSync(path.join(workspace, 'src/lib.rs'), 'utf8'), '// shared source');
    assert.equal(readFileSync(path.join(workspace, 'Cargo.lock'), 'utf8'), lock);
    assert.equal(readFileSync(path.join(workspace, 'Cargo.lock.expected'), 'utf8'), lock);
    assert.deepEqual(JSON.parse(readFileSync(path.join(workspace, 'app.json5'))), {
      app: { versionName: '0.0.1', versionCode: 7, bundleName: 'com.rssh.app' },
    });
    assert.equal(readFileSync(path.join(source, 'gen/ohos/AppScope/app.json5'), 'utf8'), appConfig);
    assert.equal(readFileSync(path.join(source, 'Cargo.toml'), 'utf8'), manifest);
    assert.equal(readFileSync(path.join(source, 'Cargo.lock'), 'utf8'), 'normal lock');
    assert.equal(JSON.parse(readFileSync(path.join(workspace, 'tauri.conf.json'))).build.frontendDist, path.join(source, 'target/ohos-frontend'));
    assert.equal(JSON.parse(readFileSync(path.join(source, 'tauri.conf.json'))).build.frontendDist, '../dist');
    prepareWorkspace(root, workspace);
    assert.equal(readFileSync(path.join(workspace, 'Cargo.toml'), 'utf8'), generated);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('release version changes keep the dependency lock valid for cargo metadata --locked', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh ohos version '));
  try {
    const source = path.join(root, 'src-tauri');
    for (const dir of ['src', 'ohos', 'gen/ohos/AppScope', 'stubs/tauri/src', 'stubs/tauri-build/src']) {
      mkdirSync(path.join(source, dir), { recursive: true });
    }
    for (const [name, version] of [['tauri', '2.11.5'], ['tauri-build', '2.6.3']]) {
      writeFileSync(path.join(source, 'stubs', name, 'Cargo.toml'), `[package]\nname = "${name}"\nversion = "${version}"\nedition = "2021"\n`);
      writeFileSync(path.join(source, 'stubs', name, 'src/lib.rs'), '');
    }
    const manifest = `[package]\nname = "rssh"\nversion = "0.6.0-rc.1"\nedition = "2021"\n[build-dependencies]\ntauri-build = { version = "2", path = ${JSON.stringify(path.join(source, 'stubs/tauri-build'))} }\n[dependencies]\ntauri = { version = "2", path = ${JSON.stringify(path.join(source, 'stubs/tauri'))} }\n`;
    const lock = 'version = 4\n\n[[package]]\nname = "rssh"\nversion = "0.0.1"\ndependencies = [\n "tauri",\n "tauri-build",\n]\n\n[[package]]\nname = "tauri"\nversion = "2.11.5"\n\n[[package]]\nname = "tauri-build"\nversion = "2.6.3"\n';
    writeFileSync(path.join(source, 'Cargo.toml'), manifest);
    writeFileSync(path.join(source, 'build.rs'), 'fn main() {}');
    writeFileSync(path.join(source, 'src/lib.rs'), '');
    writeFileSync(path.join(source, 'ohos/overlay.toml'), '[workspace]\n');
    writeFileSync(path.join(source, 'ohos/Cargo.lock'), lock);
    writeFileSync(path.join(source, 'tauri.conf.json'), JSON.stringify({ version: '0.6.0-rc.1', build: { frontendDist: '../dist' } }));
    writeFileSync(path.join(source, 'gen/ohos/AppScope/app.json5'), '{"app":{"versionName":"1.0.0","versionCode":7}}');
    const workspace = path.join(source, 'target/ohos-workspace');

    prepareWorkspace(root, workspace, { versionCode: '42' });
    const expectedLock = lock.replace('version = "0.0.1"', 'version = "0.6.0-rc.1"');
    assert.equal(readFileSync(path.join(workspace, 'Cargo.lock'), 'utf8'), expectedLock);
    assert.equal(readFileSync(path.join(source, 'ohos/Cargo.lock'), 'utf8'), lock);
    assert.equal(readFileSync(path.join(source, 'Cargo.toml'), 'utf8'), manifest);
    assert.deepEqual(JSON.parse(readFileSync(path.join(workspace, 'app.json5'))).app, {
      versionName: '0.6.0-rc.1', versionCode: 42,
    });
    const result = spawnSync('cargo', ['metadata', '--locked', '--offline', '--format-version', '1'], { cwd: workspace, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(path.join(workspace, 'Cargo.lock'), 'utf8'), expectedLock);

    prepareWorkspace(root, workspace, { versionCode: '2147483647' });
    assert.equal(JSON.parse(readFileSync(path.join(workspace, 'app.json5'))).app.versionCode, 2147483647);
    for (const versionCode of ['0', '-1', '2.5', 'abc', '2147483648', '99999999999999999999']) {
      assert.throws(() => prepareWorkspace(root, workspace, { versionCode }), /versionCode/);
    }
    writeFileSync(path.join(source, 'gen/ohos/AppScope/app.json5'), '{app:{versionName:"1.0.0",versionCode:2147483648}}');
    assert.throws(() => prepareWorkspace(root, workspace), /versionCode/);
    writeFileSync(path.join(source, 'tauri.conf.json'), JSON.stringify({ version: '0.7.0', build: { frontendDist: '../dist' } }));
    assert.throws(() => prepareWorkspace(root, workspace), /Cargo.*Tauri.*version/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
