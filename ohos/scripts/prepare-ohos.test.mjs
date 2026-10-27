import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { preparePackaging } from './prepare-ohos.mjs';

function fixture() {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh ohos '));
  const write = (relative, contents) => {
    const file = path.join(root, relative);
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, contents);
  };
  write('Cargo.toml', '[workspace]\nmembers = ["app", "core"]\nexclude = ["ohos", "target"]\nresolver = "2"\n[workspace.package]\nversion = "9.0.0"\n[workspace.dependencies]\nnative = "1"\n[patch.crates-io]\nnative = { path = "target/normal-sources/framework/native" }\n');
  for (const [platform, value] of [['normal', 1], ['ohos', 2]]) {
    const upstream = `target/${platform}-sources/framework`;
    write(`${upstream}/Cargo.toml`, '[workspace]\nmembers = ["native"]\nresolver = "2"\n[workspace.package]\nversion = "1.0.0"\n');
    write(`${upstream}/native/Cargo.toml`, '[package]\nname = "native"\nversion.workspace = true\nedition = "2021"\nlinks = "Native"\n');
    write(`${upstream}/native/build.rs`, 'fn main() {}');
    write(`${upstream}/native/src/lib.rs`, `pub const VALUE: usize = ${value};\n`);
  }
  write('app/Cargo.toml', '[package]\nname = "rssh-app"\nversion = "0.6.0-rc.1"\nedition = "2021"\n[dependencies]\nrssh-core = { path = "../core" }\nnative.workspace = true\n');
  write('core/Cargo.toml', '[package]\nname = "rssh-core"\nversion = "0.6.0-rc.1"\nedition = "2021"\n');
  write('scripts/platform-cfg.rs', 'pub fn configure() {}');
  for (const name of ['app', 'core']) {
    write(`${name}/build.rs`, '#[path = "../scripts/platform-cfg.rs"]\nmod platform;\nfn main() { platform::configure(); }');
  }
  write('app/src/lib.rs', 'pub fn answer() -> usize { rssh_core::answer() + native::VALUE }\n#[test]\nfn ordinary_app_uses_ordinary_backend() { assert_eq!(answer(), 43); }\n');
  write('core/src/lib.rs', 'pub fn answer() -> usize { 42 }\n');
  write('ohos/Cargo.toml', '[package]\nname = "rssh-ohos"\nversion = "0.0.1"\nedition = "2021"\n[workspace]\nresolver = "2"\n[dependencies]\nrssh-app = { path = "../app" }\nrssh-core = { path = "../core" }\nnative = "1"\n[patch.crates-io]\nnative = { path = "../target/ohos-sources/framework/native" }\n');
  write('ohos/src/lib.rs', '#[test]\nfn shared_app_uses_this_workspaces_native_backend() { assert_eq!(rssh_app::answer(), 44); assert_eq!(rssh_core::answer(), 42); assert_eq!(native::VALUE, 2); }\n');
  write('Cargo.lock', 'normal lock');
  write('ohos/Cargo.lock', 'OHOS lock');
  write('ohos/tauri.conf.json', JSON.stringify({ productName: 'RSSH', build: { frontendDist: '../target/ohos-frontend' } }));
  write('app/tauri.conf.json', JSON.stringify({ version: '0.6.0-rc.1', build: { frontendDist: '../dist' }, bundle: { icon: ['../icons/icon.png'] } }));
  write('ohos/app/AppScope/app.json5', '{\n// DevEco uses JSON5\napp: {versionName: "1.0.0", versionCode: 7,},\n}\n');
  return { root, write, staging: path.join(root, 'target/ohos-staging') };
}

test('packaging prepares only metadata and leaves all Cargo inputs and source configuration unchanged', (t) => {
  const { root, staging } = fixture();
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const inputs = ['Cargo.toml', 'Cargo.lock', 'app/Cargo.toml', 'core/Cargo.toml', 'ohos/Cargo.toml', 'ohos/Cargo.lock', 'ohos/tauri.conf.json', 'app/tauri.conf.json', 'ohos/app/AppScope/app.json5'];
  const originals = inputs.map((file) => readFileSync(path.join(root, file), 'utf8'));
  assert.equal(preparePackaging(root, { versionCode: '42' }), staging);
  const config = JSON.parse(readFileSync(path.join(staging, 'tauri-config.json')));
  assert.deepEqual(config, {
    version: '0.6.0-rc.1',
    build: { frontendDist: path.join(root, 'target/ohos-frontend') },
    bundle: { icon: [path.join(root, 'icons/icon.png')] },
  });
  assert.deepEqual(JSON.parse(readFileSync(path.join(staging, 'app.json5'))).app, { versionName: '0.6.0-rc.1', versionCode: 42 });
  assert.deepEqual(readdirSync(staging).sort(), ['app.json5', 'tauri-config.json']);
  assert.equal(existsSync(path.join(root, 'target/ohos-workspace')), false);
  preparePackaging(root, { versionCode: '42' });
  assert.deepEqual(inputs.map((file) => readFileSync(path.join(root, file), 'utf8')), originals);
});

test('independent workspaces reuse original app/core manifests while isolating native packages with the same links name', (t) => {
  const { root } = fixture();
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const inputs = ['Cargo.toml', 'app/Cargo.toml', 'core/Cargo.toml', 'ohos/Cargo.toml'];
  const originals = inputs.map((file) => readFileSync(path.join(root, file), 'utf8'));
  for (const [workspace, platform] of [[root, 'normal'], [path.join(root, 'ohos'), 'ohos']]) {
    rmSync(path.join(workspace, 'Cargo.lock'));
    const run = (args) => {
      const result = spawnSync('cargo', args, {
        cwd: workspace, encoding: 'utf8',
        env: { ...process.env, CARGO_TARGET_DIR: path.join(root, 'target/fixture', platform) },
      });
      assert.equal(result.status, 0, result.stdout + result.stderr);
      return result.stdout;
    };
    run(['generate-lockfile', '--offline']);
    const lock = readFileSync(path.join(workspace, 'Cargo.lock'), 'utf8');
    const metadata = JSON.parse(run(['metadata', '--locked', '--offline', '--format-version', '1']));
    assert.equal(metadata.workspace_root, realpathSync(workspace));
    for (const name of ['app', 'core']) {
      const pkg = metadata.packages.find((pkg) => pkg.name === `rssh-${name}`);
      assert.equal(pkg.manifest_path, realpathSync(path.join(root, name, 'Cargo.toml')));
      assert.ok(pkg.targets.some((target) => target.src_path === realpathSync(path.join(root, name, 'build.rs'))));
    }
    const nativePackages = metadata.packages.filter((pkg) => pkg.links === 'Native');
    assert.equal(nativePackages.length, 1);
    assert.equal(nativePackages[0].manifest_path, realpathSync(path.join(root, `target/${platform}-sources/framework/native/Cargo.toml`)));
    assert.equal(nativePackages[0].version, '1.0.0', 'upstream package inherits its own workspace version');
    run(['test', '--locked', '--offline', '--workspace', '--lib']);
    assert.equal(readFileSync(path.join(workspace, 'Cargo.lock'), 'utf8'), lock);
  }
  assert.deepEqual(inputs.map((file) => readFileSync(path.join(root, file), 'utf8')), originals);
});

test('invalid metadata fails before preparing releasable package inputs', (t) => {
  const { root, staging, write } = fixture();
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const versionCode of ['0', '-1', '2.5', 'abc', '2147483648', '99999999999999999999']) {
    assert.throws(() => preparePackaging(root, { versionCode }), /versionCode/);
    assert.equal(existsSync(staging), false);
  }
  preparePackaging(root, { versionCode: '2147483647' });
  assert.equal(JSON.parse(readFileSync(path.join(staging, 'app.json5'))).app.versionCode, 2147483647);
  write('ohos/app/AppScope/app.json5', '{app:{versionName:"1.0.0",versionCode:2147483648}}');
  assert.throws(() => preparePackaging(root), /versionCode/);
  write('app/tauri.conf.json', JSON.stringify({ version: '0.7.0', build: { frontendDist: '../dist' } }));
  assert.throws(() => preparePackaging(root), /Cargo.*Tauri.*version/);
});
