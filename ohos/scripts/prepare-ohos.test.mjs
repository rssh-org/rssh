import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { prepareWorkspace } from './prepare-ohos.mjs';

function fixture() {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh ohos '));
  const write = (relative, contents) => {
    const file = path.join(root, relative);
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, contents);
  };
  write('Cargo.toml', '[workspace]\nmembers = ["app", "core"]\nresolver = "2"\n');
  write('Cargo.lock', 'normal lock');
  for (const [name, version] of [['tauri', '2.11.5'], ['tauri-build', '2.6.3']]) {
    write(`stubs/${name}/Cargo.toml`, `[package]\nname = "${name}"\nversion = "${version}"\nedition = "2021"\n[workspace]\n`);
    write(`stubs/${name}/src/lib.rs`, '');
  }
  write('app/Cargo.toml', `[package]\nname = "rssh-app"\nversion = "0.6.0-rc.1"\nedition = "2021"\n[lib]\nname = "rssh_lib"\n[build-dependencies]\ntauri-build = { version = "2", path = ${JSON.stringify(path.join(root, 'stubs/tauri-build'))} }\n[dependencies]\nrssh-core = { path = "../core" }\ntauri = { version = "2", path = ${JSON.stringify(path.join(root, 'stubs/tauri'))} }\n`);
  write('core/Cargo.toml', '[package]\nname = "rssh-core"\nversion = "0.6.0-rc.1"\nedition = "2021"\n');
  write('scripts/platform-cfg.rs', 'pub fn configure() {}');
  for (const name of ['app', 'core']) {
    write(`${name}/build.rs`, '#[path = "../scripts/platform-cfg.rs"]\nmod platform;\nfn main() { platform::configure(); }');
  }
  write('app/src/lib.rs', '#[path = "../../ohos/src/mod.rs"]\nmod ohos;\npub use ohos::answer;\n');
  write('core/src/lib.rs', '#[path = "../../ohos/src/core.rs"]\nmod ohos;\npub use ohos::answer;\n');
  write('ohos/src/mod.rs', 'pub fn answer() -> usize { rssh_core::answer() }\n');
  write('ohos/src/core.rs', 'pub fn answer() -> usize { 42 }\n');
  write('ohos/framework/overlay.toml', '[workspace]\nmembers = ["core"]\nresolver = "2"\n');
  write('ohos/framework/core.toml', '');
  const lock = 'version = 4\n\n[[package]]\nname = "rssh-app"\nversion = "0.0.1"\ndependencies = [\n "rssh-core",\n "tauri",\n "tauri-build",\n]\n\n[[package]]\nname = "rssh-core"\nversion = "0.0.1"\n\n[[package]]\nname = "tauri"\nversion = "2.11.5"\n\n[[package]]\nname = "tauri-build"\nversion = "2.6.3"\n';
  write('ohos/framework/Cargo.lock', lock);
  write('app/tauri.conf.json', JSON.stringify({ version: '0.6.0-rc.1', build: { frontendDist: '../dist' }, bundle: { icon: ['../icons/icon.png'] } }));
  write('ohos/app/AppScope/app.json5', '{\n// DevEco uses JSON5\napp: {versionName: "1.0.0", versionCode: 7,},\n}\n');
  return { root, write, lock, workspace: path.join(root, 'target/ohos-workspace') };
}

test('isolated app/core graph compiles original sources and build scripts without modifying the normal workspace', () => {
  const { root, workspace, lock } = fixture();
  const inputs = ['Cargo.lock', 'app/Cargo.toml', 'core/Cargo.toml', 'app/tauri.conf.json', 'ohos/app/AppScope/app.json5', 'ohos/framework/Cargo.lock'];
  const originals = inputs.map((file) => readFileSync(path.join(root, file), 'utf8'));
  try {
    prepareWorkspace(root, workspace, { versionCode: '42' });
    const generated = readFileSync(path.join(workspace, 'Cargo.toml'), 'utf8');
    assert.match(generated, /tauri-build = \{ version = "=2\.6\.3"/);
    assert.match(generated, /tauri = \{ version = "=2\.11\.5"/);
    assert.match(generated, /rssh-core = \{ path = "core"/);
    for (const [name, manifestPath] of [['app', 'Cargo.toml'], ['core', 'core/Cargo.toml']]) {
      const manifest = readFileSync(path.join(workspace, manifestPath), 'utf8');
      assert.ok(manifest.includes(`path = ${JSON.stringify(path.join(root, name, 'src/lib.rs'))}`));
      assert.ok(manifest.includes(`build = ${JSON.stringify(path.join(root, name, 'build.rs'))}`));
    }
    const config = JSON.parse(readFileSync(path.join(workspace, 'tauri.conf.json')));
    assert.equal(config.bundle.icon[0], path.join(root, 'icons/icon.png'));
    assert.equal(config.build.frontendDist, path.join(root, 'target/ohos-frontend'));
    assert.deepEqual(JSON.parse(readFileSync(path.join(workspace, 'app.json5'))).app, { versionName: '0.6.0-rc.1', versionCode: 42 });
    const expectedLock = lock.replaceAll('version = "0.0.1"', 'version = "0.6.0-rc.1"');
    assert.equal(readFileSync(path.join(workspace, 'Cargo.lock'), 'utf8'), expectedLock);
    assert.equal(readFileSync(path.join(workspace, 'Cargo.lock.expected'), 'utf8'), expectedLock);
    const metadata = spawnSync('cargo', ['metadata', '--locked', '--offline', '--format-version', '1'], { cwd: workspace, encoding: 'utf8' });
    assert.equal(metadata.status, 0, metadata.stderr);
    const packages = JSON.parse(metadata.stdout).packages;
    assert.equal(packages.find((pkg) => pkg.name === 'rssh-core').manifest_path, realpathSync(path.join(workspace, 'core/Cargo.toml')));
    const compiled = spawnSync('cargo', ['check', '--locked', '--offline', '--workspace', '--lib'], { cwd: workspace, encoding: 'utf8' });
    assert.equal(compiled.status, 0, compiled.stderr);
    assert.equal(readFileSync(path.join(workspace, 'Cargo.lock'), 'utf8'), expectedLock);
    prepareWorkspace(root, workspace, { versionCode: '42' });
    assert.equal(readFileSync(path.join(workspace, 'Cargo.toml'), 'utf8'), generated);
    assert.deepEqual(inputs.map((file) => readFileSync(path.join(root, file), 'utf8')), originals);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('app and core overlays resolve pinned native sources from root target', () => {
  const { root, workspace, write } = fixture();
  try {
    write('ohos/framework/overlay.toml', '[workspace]\nmembers = ["core"]\n[patch.crates-io]\nwry = { path = "@OHOS_SOURCES@/wry" }\n');
    write('ohos/framework/core.toml', '[target.\'cfg(target_env = "ohos")\'.dependencies]\nopenharmony-ability = { path = "@OHOS_SOURCES@/ability/crates/ability" }\n');
    prepareWorkspace(root, workspace);
    for (const [manifestPath, dependency] of [['Cargo.toml', 'wry'], ['core/Cargo.toml', 'ability/crates/ability']]) {
      const manifest = readFileSync(path.join(workspace, manifestPath), 'utf8');
      assert.ok(manifest.includes(JSON.stringify(path.join(root, 'target/ohos-sources', dependency))));
      assert.doesNotMatch(manifest, /@OHOS_SOURCES@/);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('invalid package metadata fails before generating a releasable workspace', () => {
  const { root, workspace, write } = fixture();
  try {
    prepareWorkspace(root, workspace, { versionCode: '2147483647' });
    assert.equal(JSON.parse(readFileSync(path.join(workspace, 'app.json5'))).app.versionCode, 2147483647);
    for (const versionCode of ['0', '-1', '2.5', 'abc', '2147483648', '99999999999999999999']) {
      assert.throws(() => prepareWorkspace(root, workspace, { versionCode }), /versionCode/);
    }
    write('ohos/app/AppScope/app.json5', '{app:{versionName:"1.0.0",versionCode:2147483648}}');
    assert.throws(() => prepareWorkspace(root, workspace), /versionCode/);
    write('app/tauri.conf.json', JSON.stringify({ version: '0.7.0', build: { frontendDist: '../dist' } }));
    assert.throws(() => prepareWorkspace(root, workspace), /Cargo.*Tauri.*version/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
