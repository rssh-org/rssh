import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { prepareRelease, releaseVersion } from './prepare-release.mjs';

function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh release '));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const write = (file, value) => {
    mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
    writeFileSync(path.join(root, file), value);
  };
  write('Cargo.toml', '[workspace]\nmembers = ["core", "app", "cli"]\nexclude = ["ohos"]\nresolver = "2"\n');
  for (const name of ['core', 'app', 'cli', 'ohos']) {
    write(`${name}/Cargo.toml`, `[package]\nname = "rssh-${name}"\nversion = "0.0.1"\nedition = "2021"\n` +
      (name === 'ohos' ? '[workspace]\nresolver = "2"\n' : '') +
      (name === 'core' ? '' : '[dependencies]\nrssh-core = { path = "../core" }\n') +
      (name === 'ohos' ? 'rssh-app = { path = "../app" }\n' : ''));
    write(`${name}/src/lib.rs`, 'pub fn ready() {}');
  }
  for (const manifest of ['Cargo.toml', 'ohos/Cargo.toml']) {
    const result = spawnSync('cargo', ['generate-lockfile', '--offline', '--manifest-path', path.join(root, manifest)], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
  }
  write('package.json', JSON.stringify({ name: 'rssh', version: '0.0.1' }));
  write('package-lock.json', JSON.stringify({ version: '0.0.1', packages: { '': { version: '0.0.1' }, 'node_modules/example': { version: '7.0.0' } } }));
  write('app/tauri.conf.json', JSON.stringify({ version: '0.0.1', identifier: 'com.rssh.app' }));
  return { root, write };
}

test('stable tags map to ordered version codes, with explicit override for existing AGC history', () => {
  assert.deepEqual(releaseVersion('v0.3.2'), { version: '0.3.2', versionCode: 3002 });
  assert.deepEqual(releaseVersion('1.2.3', '1800000000'), { version: '1.2.3', versionCode: 1800000000 });
  assert.ok(releaseVersion('1.0.0').versionCode > releaseVersion('0.999.999').versionCode);
  for (const version of ['v1.0.0-beta.1', 'v1.0', 'v01.2.3', '$(touch /tmp/oops)', '0.0.0', '2148.0.0', '1.1000.0']) {
    assert.throws(() => releaseVersion(version));
  }
  for (const code of ['0', '-1', '1.5', '2147483648']) assert.throws(() => releaseVersion('1.0.0', code));
});

test('release preparation updates own versions in both workspaces without unlocking dependencies', (t) => {
  const { root } = fixture(t);
  assert.equal(prepareRelease(root, 'v0.3.2').versionCode, 3002);
  for (const manifest of ['Cargo.toml', 'ohos/Cargo.toml']) {
    const result = spawnSync('cargo', ['metadata', '--locked', '--offline', '--format-version', '1', '--manifest-path', path.join(root, manifest)], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    for (const pkg of JSON.parse(result.stdout).packages) assert.equal(pkg.version, '0.3.2');
  }
  const npmLock = JSON.parse(readFileSync(path.join(root, 'package-lock.json')));
  assert.equal(npmLock.version, '0.3.2');
  assert.equal(npmLock.packages[''].version, '0.3.2');
  assert.equal(npmLock.packages['node_modules/example'].version, '7.0.0');
  assert.equal(JSON.parse(readFileSync(path.join(root, 'app/tauri.conf.json'))).identifier, 'com.rssh.app');
  const lock = readFileSync(path.join(root, 'ohos/Cargo.lock'), 'utf8');
  prepareRelease(root, 'v0.3.2');
  assert.equal(readFileSync(path.join(root, 'ohos/Cargo.lock'), 'utf8'), lock);
});

test('unexpected or missing path packages fail before changing files', (t) => {
  const { root, write } = fixture(t);
  const before = readFileSync(path.join(root, 'app/Cargo.toml'), 'utf8');
  write('ohos/Cargo.lock', 'version = 4\n');
  assert.throws(() => prepareRelease(root, 'v1.0.0'), /lockfile/);
  assert.equal(readFileSync(path.join(root, 'app/Cargo.toml'), 'utf8'), before);
});
