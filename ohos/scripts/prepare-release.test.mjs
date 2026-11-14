import assert from 'node:assert/strict';
import { generateKeyPairSync } from 'node:crypto';
import { mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { prepareRelease, releaseVersion } from './prepare-release.mjs';

function fixture(t) {
  const root = realpathSync(mkdtempSync(path.join(tmpdir(), 'rssh release ')));
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
  for (const script of ['prepare-release.mjs', 'upload-agc.mjs', 'release-notes.mjs']) {
    write(`ohos/scripts/${script}`, readFileSync(new URL(script, import.meta.url), 'utf8'));
  }
  return { root, write };
}

test('stable tags use the supplied version code without deriving it from semver', () => {
  assert.deepEqual(releaseVersion('v0.3.2', '42'), { version: '0.3.2', versionCode: 42 });
  assert.deepEqual(releaseVersion('1.2.3', '1800000000'), { version: '1.2.3', versionCode: 1800000000 });
  assert.deepEqual(releaseVersion('1.1000.0', 43), { version: '1.1000.0', versionCode: 43 });
  assert.equal(releaseVersion('0.0.0', '1').versionCode, 1);
  assert.equal(releaseVersion('2148.0.0', '2147483647').versionCode, 2147483647);
  for (const version of ['v1.0.0-beta.1', 'v1.0', 'v01.2.3', '$(touch /tmp/oops)']) {
    assert.throws(() => releaseVersion(version, '42'));
  }
  for (const code of [undefined, '', null, ' ', '0', '-1', '1.5', '01', '1e3', '2147483648']) {
    assert.throws(() => releaseVersion('1.0.0', code));
  }
});

test('release preparation updates own versions in both workspaces without unlocking dependencies', (t) => {
  const { root } = fixture(t);
  assert.equal(prepareRelease(root, 'v0.3.2', 42).versionCode, 42);
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
  prepareRelease(root, 'v0.3.2', 42);
  assert.equal(readFileSync(path.join(root, 'ohos/Cargo.lock'), 'utf8'), lock);
});

test('unexpected or missing path packages fail before changing files', (t) => {
  const { root, write } = fixture(t);
  const before = readFileSync(path.join(root, 'app/Cargo.toml'), 'utf8');
  write('ohos/Cargo.lock', 'version = 4\n');
  assert.throws(() => prepareRelease(root, 'v1.0.0', 42), /lockfile/);
  assert.equal(readFileSync(path.join(root, 'app/Cargo.toml'), 'utf8'), before);
});

test('a missing or invalid version code fails before changing release files', (t) => {
  const { root } = fixture(t);
  const before = readFileSync(path.join(root, 'package.json'), 'utf8');
  for (const code of [undefined, '', '2147483648']) {
    assert.throws(() => prepareRelease(root, 'v1.0.0', code), /versionCode|OHOS_VERSION_CODE/);
    assert.equal(readFileSync(path.join(root, 'package.json'), 'utf8'), before);
  }
});

test('the release CLI requires manual input when AGC lookup cannot authenticate', (t) => {
  const { root } = fixture(t);
  const run = spawnSync(process.execPath, [path.join(root, 'ohos/scripts/prepare-release.mjs'), 'v1.0.0'], {
    encoding: 'utf8',
    env: { ...process.env, CI: 'true', OHOS_VERSION_CODE: '', AGC_APP_ID: '', AGC_SERVICE_ACCOUNT_JSON: '', GITHUB_ENV: path.join(root, 'github-env') },
  });
  assert.equal(run.status, 1);
  assert.match(run.stderr, /version_code/);
  assert.equal(JSON.parse(readFileSync(path.join(root, 'package.json'))).version, '0.0.1');
});

test('the release CLI exports an explicit version code without AGC credentials', (t) => {
  const { root } = fixture(t);
  const envFile = path.join(root, 'github-env');
  const run = spawnSync(process.execPath, [path.join(root, 'ohos/scripts/prepare-release.mjs'), 'v0.3.2'], {
    encoding: 'utf8',
    env: { ...process.env, CI: 'true', OHOS_VERSION_CODE: '42', AGC_APP_ID: '', AGC_SERVICE_ACCOUNT_JSON: '', GITHUB_ENV: envFile },
  });
  assert.equal(run.status, 0, run.stderr);
  assert.equal(readFileSync(envFile, 'utf8'), 'OHOS_RELEASE_VERSION=0.3.2\nOHOS_VERSION_CODE=42\n');
  assert.equal(JSON.parse(readFileSync(path.join(root, 'package.json'))).version, '0.3.2');
});

test('the release CLI queries AGC and exports the next code for packaging', (t) => {
  const { root, write } = fixture(t);
  const envFile = path.join(root, 'github-env');
  const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const serviceAccountJson = JSON.stringify({
    key_id: 'test-key', sub_account: 'test-developer',
    private_key: privateKey.export({ type: 'pkcs8', format: 'pem' }),
  });
  write('fetch-fixture.mjs', `
    import assert from 'node:assert/strict';
    globalThis.fetch = async (url, options) => {
      assert.equal(String(url), 'https://connect-api.cloud.huawei.com/api/publish/v3/version/brief-info/list');
      assert.equal(options.method, 'POST');
      assert.equal(options.headers.appId, '123456789');
      return new Response(JSON.stringify({ ret: { code: 0 }, versionList: [
        { packageList: [{ versionCode: 7001 }] },
      ] }), { status: 200 });
    };
  `);
  const run = spawnSync(process.execPath, [
    '--import', path.join(root, 'fetch-fixture.mjs'), path.join(root, 'ohos/scripts/prepare-release.mjs'), 'v0.3.2',
  ], {
    encoding: 'utf8',
    env: { ...process.env, CI: 'true', OHOS_VERSION_CODE: '', AGC_APP_ID: '123456789', AGC_SERVICE_ACCOUNT_JSON: serviceAccountJson, GITHUB_ENV: envFile },
  });
  assert.equal(run.status, 0, run.stderr);
  assert.equal(readFileSync(envFile, 'utf8'), 'OHOS_RELEASE_VERSION=0.3.2\nOHOS_VERSION_CODE=7002\n');
});
