import assert from 'node:assert/strict';
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const repo = fileURLToPath(new URL('..', import.meta.url));

function runBuild(failure, { version = '0.0.1', versionCode = '', args = [], targetDir = '' } = {}) {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh ohos build '));
  const source = path.join(root, 'src-tauri');
  const packagePaths = Object.fromEntries(['signed', 'unsigned'].flatMap((signature) => [
    [`phone-${signature}`, `products/phone/build/default/outputs/default/entry-default-${signature}.hap`],
    [`desktop-${signature}`, `products/desktop/build/default/outputs/default/desktop-default-${signature}.hap`],
    [`app-${signature}`, `build/outputs/default/RSSH${signature === 'unsigned' ? '-unsigned' : ''}.app`],
  ]).map(([key, relativePath]) => [key, path.join(source, 'gen/ohos', relativePath)]));
  const legacyHap = path.join(source, 'gen/ohos/entry/build/default/outputs/default/entry-default-signed.hap');
  const tools = path.join(root, 'fake-tools');
  const sdk = path.join(root, 'sdk/default/openharmony');
  for (const dir of ['scripts', 'dist', 'fake-tools', 'src-tauri/ohos', 'src-tauri/src', 'src-tauri/gen/ohos/AppScope', 'src-tauri/target/ohos-staging/arm64-v8a', 'sdk/default/openharmony/native/llvm/bin']) {
    mkdirSync(path.join(root, dir), { recursive: true });
  }
  copyFileSync(path.join(repo, 'build-ohos.sh'), path.join(root, 'build-ohos.sh'));
  copyFileSync(path.join(repo, 'scripts/prepare-ohos.mjs'), path.join(root, 'scripts/prepare-ohos.mjs'));
  copyFileSync(path.join(repo, 'scripts/prepare-ohos-sources.mjs'), path.join(root, 'scripts/prepare-ohos-sources.mjs'));
  symlinkSync(path.join(repo, 'node_modules'), path.join(root, 'node_modules'), process.platform === 'win32' ? 'junction' : 'dir');
  writeFileSync(path.join(source, 'ohos/sources.json'), JSON.stringify({
    repositories: failure === 'sources' ? { invalid: { rev: 'unfixed branch' } } : {}, abilityModules: [],
  }));
  writeFileSync(path.join(source, 'Cargo.toml'), `[package]\nname = "rssh"\nversion = "${version}"\n[build-dependencies]\ntauri-build = { version = "2" }\n[dependencies]\ntauri = { version = "2" }\n`);
  writeFileSync(path.join(source, 'Cargo.lock'), 'normal lock');
  const lock = 'version = 4\n\n[[package]]\nname = "rssh"\nversion = "0.0.1"\n';
  writeFileSync(path.join(source, 'ohos/Cargo.lock'), lock);
  writeFileSync(path.join(source, 'ohos/overlay.toml'), '[workspace]\n');
  writeFileSync(path.join(source, 'build.rs'), 'fn main() {}');
  writeFileSync(path.join(source, 'tauri.conf.json'), JSON.stringify({ version, build: { frontendDist: '../dist' } }));
  const appConfigPath = path.join(source, 'gen/ohos/AppScope/app.json5');
  const appConfig = '{\n// Existing DevEco project configuration\napp: {bundleName: "com.rssh.app", versionName: "old-name", versionCode: 9,},\n}\n';
  writeFileSync(appConfigPath, appConfig);
  writeFileSync(path.join(root, 'dist/index.html'), 'old frontend');
  writeFileSync(path.join(source, 'target/ohos-staging/arm64-v8a/librssh_lib.so'), 'old library');
  for (const packagePath of [...Object.values(packagePaths), legacyHap]) {
    mkdirSync(path.dirname(packagePath), { recursive: true });
    writeFileSync(packagePath, 'old package');
  }
  const fakeTool = `#!${process.execPath}
const fs = require('node:fs');
const path = require('node:path');
const tool = path.basename(process.argv[1]);
const args = process.argv.slice(2);
const root = process.env.FIXTURE_ROOT;
fs.appendFileSync(path.join(root, 'calls'), tool + ' ' + args.join(' ') + '\\n');
if (tool === 'rustup') { console.log('aarch64-unknown-linux-ohos'); process.exit(0); }
if (tool === 'ohrs' && args[0] === '--version') { console.log('Version: 1.5.0'); process.exit(0); }
if (process.env.FAIL_AT === tool) process.exit(7);
if (tool === 'npm') {
  const frontend = args[args.indexOf('--outDir') + 1];
  fs.mkdirSync(frontend, { recursive: true });
  fs.writeFileSync(path.join(frontend, 'index.html'), 'new frontend');
}
if (tool === 'ohrs' && process.env.FAIL_AT !== 'missing-artifact') {
  const dist = args[args.indexOf('--dist') + 1];
  fs.mkdirSync(path.join(dist, 'arm64-v8a'), { recursive: true });
  fs.writeFileSync(path.join(dist, 'arm64-v8a/librssh_lib.so'), 'new library');
  if (process.env.FAIL_AT === 'mutated-lock') fs.appendFileSync('Cargo.lock', '\\n# unexpected lock mutation\\n');
}
if (tool === 'hvigor.js') {
  if (process.env.FAIL_AT === 'missing-package') process.exit(0);
  if (fs.readFileSync(path.join(root, 'src-tauri/gen/ohos/common/runtime/libs/arm64-v8a/librssh_lib.so'), 'utf8') !== 'new library') process.exit(9);
  if (fs.readFileSync(path.join(root, 'src-tauri/gen/ohos/common/runtime/src/main/resources/rawfile/index.html'), 'utf8') !== 'new frontend') process.exit(9);
  fs.copyFileSync(path.join(root, 'src-tauri/gen/ohos/AppScope/app.json5'), path.join(root, 'hap-app-config.json'));
  const signature = args.includes('enableSignTask=false') || process.env.FAIL_AT === 'unsigned-only' ? 'unsigned' : 'signed';
  const outputs = {
    phone: 'products/phone/build/default/outputs/default/entry-default-' + signature + '.hap',
    desktop: 'products/desktop/build/default/outputs/default/desktop-default-' + signature + '.hap',
    app: 'build/outputs/default/RSSH' + (signature === 'unsigned' ? '-unsigned' : '') + '.app',
  };
  for (const [name, relativePath] of Object.entries(outputs)) {
    if (process.env.FAIL_AT === 'missing-' + name) continue;
    const output = path.join(root, 'src-tauri/gen/ohos', relativePath);
    fs.mkdirSync(path.dirname(output), { recursive: true });
    fs.writeFileSync(output, 'new ' + name + ' package');
  }
}
`;
  for (const name of ['rustup', 'cargo', 'npm', 'ohrs', 'ohpm', 'hvigor.js']) {
    writeFileSync(path.join(tools, name), fakeTool);
    chmodSync(path.join(tools, name), 0o755);
  }
  for (const name of ['clang', 'clang++', 'llvm-ar']) {
    const file = path.join(sdk, 'native/llvm/bin', name);
    writeFileSync(file, '#!/bin/sh\nexit 0\n');
    chmodSync(file, 0o755);
  }
  try {
    const result = spawnSync('bash', [path.join(root, 'build-ohos.sh'), ...args], {
      encoding: 'utf8',
      env: { ...process.env, PATH: `${tools}${path.delimiter}${process.env.PATH}`, FIXTURE_ROOT: root, FAIL_AT: failure,
        CARGO_TARGET_DIR: targetDir, OHOS_HOME: sdk, OHOS_VERSION_CODE: versionCode,
        HVIGOR_BIN: path.join(tools, 'hvigor.js'), OHPM_BIN: path.join(tools, 'ohpm') },
    });
    assert.equal(readFileSync(path.join(source, 'Cargo.lock'), 'utf8'), 'normal lock');
    assert.equal(readFileSync(path.join(source, 'ohos/Cargo.lock'), 'utf8'), lock);
    assert.equal(readFileSync(path.join(root, 'dist/index.html'), 'utf8'), 'old frontend');
    assert.equal(readFileSync(appConfigPath, 'utf8'), appConfig, 'source AppScope must be restored on both success and failure');
    const packagedConfig = path.join(root, 'hap-app-config.json');
    return {
      ...result,
      calls: existsSync(path.join(root, 'calls')) ? readFileSync(path.join(root, 'calls'), 'utf8') : '',
      packages: Object.fromEntries(Object.entries(packagePaths).map(([key, packagePath]) => [
        key, existsSync(packagePath) ? readFileSync(packagePath, 'utf8') : null,
      ])),
      legacyHap: existsSync(legacyHap) ? readFileSync(legacyHap, 'utf8') : null,
      packagedApp: existsSync(packagedConfig) ? JSON.parse(readFileSync(packagedConfig)).app : null,
    };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

for (const failure of ['npm', 'sources', 'cargo', 'ohrs', 'missing-artifact', 'mutated-lock', 'ohpm']) {
  test(`${failure} failure stops before App Pack assembly and discards all stale packages`, () => {
    const result = runBuild(failure);
    assert.notEqual(result.status, 0, result.stdout + result.stderr);
    assert.doesNotMatch(result.calls, /hvigor\.js assembleApp/);
    assert.ok(Object.values(result.packages).every((value) => value === null));
    assert.equal(result.legacyHap, null);
    assert.doesNotMatch(result.stdout, /Built /);
  });
}

for (const failure of ['hvigor.js', 'missing-package', 'missing-phone', 'missing-desktop', 'missing-app']) {
  test(`${failure} cannot report a successful build`, () => {
    const result = runBuild(failure);
    assert.notEqual(result.status, 0, result.stdout + result.stderr);
    assert.equal(result.legacyHap, null);
    assert.doesNotMatch(result.stdout, /Built /);
  });
}

test('success builds the common runtime once and assembles both signed HAPs and one App Pack', () => {
  const result = runBuild('');
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.calls, /cargo metadata --locked/);
  assert.match(result.calls, /ohrs build .*--locked --lib --features custom-protocol/);
  assert.match(result.calls, /--target-dir .*\/src-tauri\/target\/ohos --/);
  assert.equal((result.calls.match(/^npm run build /gm) ?? []).length, 1);
  assert.equal((result.calls.match(/^ohrs build /gm) ?? []).length, 1);
  assert.match(result.calls, /hvigor\.js assembleApp --mode project -p product=default -p buildMode=release -p enableSignTask=true/);
  assert.equal(result.packages['phone-signed'], 'new phone package');
  assert.equal(result.packages['desktop-signed'], 'new desktop package');
  assert.equal(result.packages['app-signed'], 'new app package');
  assert.equal(result.legacyHap, null);
  assert.deepEqual(result.packagedApp, { bundleName: 'com.rssh.app', versionName: '0.0.1', versionCode: 9 });
  assert.match(result.stdout, /Built phone\/tablet HAP:/);
  assert.match(result.stdout, /Built desktop HAP:/);
  assert.match(result.stdout, /Built App Pack:/);
});

test('release packaging rejects unsigned fallback even when unsigned files exist', () => {
  const result = runBuild('unsigned-only');
  assert.notEqual(result.status, 0, result.stdout + result.stderr);
  assert.equal(result.packages['phone-signed'], null);
  assert.equal(result.packages['phone-unsigned'], 'new phone package');
  assert.match(result.stderr, /Configure signing/);
  assert.doesNotMatch(result.stdout, /Built /);
});

test('explicit unsigned packaging disables signing and still builds release code', () => {
  const result = runBuild('', { args: ['--unsigned'] });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.calls, /hvigor\.js assembleApp --mode project -p product=default -p buildMode=release -p enableSignTask=false/);
  assert.doesNotMatch(result.calls, /devtools/);
  for (const name of ['phone', 'desktop', 'app']) {
    assert.equal(result.packages[`${name}-unsigned`], `new ${name} package`);
    assert.equal(result.packages[`${name}-signed`], null);
  }
});

test('respects an explicit Cargo target directory for cached OHOS builds', () => {
  const targetDir = path.join(tmpdir(), 'rssh cached ohos target');
  const result = runBuild('', { targetDir });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.ok(result.calls.includes(`--target-dir ${targetDir} --`));
});

test('unknown options stop before the build starts', () => {
  const result = runBuild('', { args: ['--signing-optional'] });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Unknown option/);
  assert.equal(result.calls, '');
});

test('release packaging follows Tauri version and explicit build code without modifying sources', () => {
  const result = runBuild('', { version: '0.7.0-rc.2', versionCode: '23' });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.deepEqual(result.packagedApp, { bundleName: 'com.rssh.app', versionName: '0.7.0-rc.2', versionCode: 23 });
});

test('invalid build code cannot assemble or report a package', () => {
  const result = runBuild('', { versionCode: '1.2' });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /versionCode/);
  assert.doesNotMatch(result.calls, /hvigor\.js assembleApp/);
  assert.ok(Object.values(result.packages).every((value) => value === null));
});
