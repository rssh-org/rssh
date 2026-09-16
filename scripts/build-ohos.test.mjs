import assert from 'node:assert/strict';
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const repo = fileURLToPath(new URL('..', import.meta.url));

function runBuild(failure) {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh ohos build '));
  const source = path.join(root, 'src-tauri');
  const hap = path.join(source, 'gen/ohos/entry/build/default/outputs/default/entry-default-signed.hap');
  const tools = path.join(root, 'fake-tools');
  const sdk = path.join(root, 'sdk/default/openharmony');
  for (const dir of ['scripts', 'dist', 'fake-tools', 'src-tauri/ohos', 'src-tauri/src', 'src-tauri/gen/ohos', 'src-tauri/target/ohos-staging/arm64-v8a', 'sdk/default/openharmony/native/llvm/bin']) {
    mkdirSync(path.join(root, dir), { recursive: true });
  }
  mkdirSync(path.dirname(hap), { recursive: true });
  copyFileSync(path.join(repo, 'build-ohos.sh'), path.join(root, 'build-ohos.sh'));
  copyFileSync(path.join(repo, 'scripts/prepare-ohos.mjs'), path.join(root, 'scripts/prepare-ohos.mjs'));
  copyFileSync(path.join(repo, 'scripts/prepare-ohos-sources.mjs'), path.join(root, 'scripts/prepare-ohos-sources.mjs'));
  writeFileSync(path.join(source, 'ohos/sources.json'), JSON.stringify({
    repositories: failure === 'sources' ? { invalid: { rev: 'unfixed branch' } } : {}, abilityModules: [],
  }));
  writeFileSync(path.join(source, 'Cargo.toml'), '[package]\nname = "rssh"\nversion = "0.0.1"\n[build-dependencies]\ntauri-build = { version = "2" }\n[dependencies]\ntauri = { version = "2" }\n');
  writeFileSync(path.join(source, 'Cargo.lock'), 'normal lock');
  writeFileSync(path.join(source, 'ohos/Cargo.lock'), 'ohos lock');
  writeFileSync(path.join(source, 'ohos/overlay.toml'), '[workspace]\n');
  writeFileSync(path.join(source, 'build.rs'), 'fn main() {}');
  writeFileSync(path.join(source, 'tauri.conf.json'), '{"build":{"frontendDist":"../dist"}}');
  writeFileSync(path.join(root, 'dist/index.html'), 'old frontend');
  writeFileSync(path.join(source, 'target/ohos-staging/arm64-v8a/librssh_lib.so'), 'old library');
  writeFileSync(hap, 'old package');
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
}
if (tool === 'hvigor.js') {
  if (process.env.FAIL_AT === 'missing-package') process.exit(0);
  if (fs.readFileSync(path.join(root, 'src-tauri/gen/ohos/entry/libs/arm64-v8a/librssh_lib.so'), 'utf8') !== 'new library') process.exit(9);
  if (fs.readFileSync(path.join(root, 'src-tauri/gen/ohos/entry/src/main/resources/rawfile/index.html'), 'utf8') !== 'new frontend') process.exit(9);
  const hap = path.join(root, 'src-tauri/gen/ohos/entry/build/default/outputs/default/entry-default-signed.hap');
  fs.mkdirSync(path.dirname(hap), { recursive: true });
  fs.writeFileSync(hap, 'new package');
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
    const result = spawnSync('bash', [path.join(root, 'build-ohos.sh')], {
      encoding: 'utf8',
      env: { ...process.env, PATH: `${tools}${path.delimiter}${process.env.PATH}`, FIXTURE_ROOT: root, FAIL_AT: failure,
        OHOS_HOME: sdk, HVIGOR_BIN: path.join(tools, 'hvigor.js'), OHPM_BIN: path.join(tools, 'ohpm') },
    });
    assert.equal(readFileSync(path.join(source, 'Cargo.lock'), 'utf8'), 'normal lock');
    assert.equal(readFileSync(path.join(root, 'dist/index.html'), 'utf8'), 'old frontend');
    return { ...result, calls: readFileSync(path.join(root, 'calls'), 'utf8'), hap: existsSync(hap) ? readFileSync(hap, 'utf8') : null };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

for (const failure of ['npm', 'sources', 'cargo', 'ohrs', 'missing-artifact', 'ohpm']) {
  test(`${failure} failure stops before HAP assembly and discards stale package`, () => {
    const result = runBuild(failure);
    assert.notEqual(result.status, 0, result.stdout + result.stderr);
    assert.doesNotMatch(result.calls, /hvigor\.js assembleHap/);
    assert.equal(result.hap, null);
    assert.doesNotMatch(result.stdout, /Built:/);
  });
}

for (const failure of ['hvigor.js', 'missing-package']) {
  test(`${failure} cannot report a successful build`, () => {
    const result = runBuild(failure);
    assert.notEqual(result.status, 0, result.stdout + result.stderr);
    assert.equal(result.hap, null);
    assert.doesNotMatch(result.stdout, /Built:/);
  });
}

test('success assembles only after a locked Rust build and stages the current library', () => {
  const result = runBuild('');
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.calls, /cargo metadata --locked/);
  assert.match(result.calls, /ohrs build .*--locked --lib --features custom-protocol/);
  assert.match(result.calls, /hvigor\.js assembleHap/);
  assert.equal(result.hap, 'new package');
  assert.match(result.stdout, /Built:/);
});
