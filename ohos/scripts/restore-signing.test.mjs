import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('../..', import.meta.url));
const password = 'a1'.repeat(32);
const config = { signingConfigs: [{ name: 'default', type: 'HarmonyOS', material: {
  storeFile: 'signing/release.p12', profile: 'signing/release.p7b', certpath: 'signing/release.cer',
  storePassword: password, keyPassword: password, keyAlias: 'fixture', signAlg: 'SHA256withECDSA',
} }] };

function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh signing '));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const app = path.join(root, 'ohos/app');
  const scripts = path.join(root, 'ohos/scripts');
  const inputs = path.join(root, 'inputs');
  for (const directory of [app, scripts, path.join(inputs, 'signing/material')]) mkdirSync(directory, { recursive: true });
  const script = path.join(scripts, 'restore-signing.py');
  copyFileSync(path.join(repo, 'ohos/scripts/restore-signing.py'), script);
  writeFileSync(path.join(inputs, 'signing.local.json'), JSON.stringify(config));
  for (const name of ['release.p12', 'release.p7b', 'release.cer', 'material/key']) {
    writeFileSync(path.join(inputs, 'signing', name), `fixture: ${name}`);
  }
  return {
    root, app, inputs,
    archive() {
      return execFileSync('tar', ['-czf', '-', '-C', inputs, 'signing.local.json', 'signing'], {
        env: { ...process.env, COPYFILE_DISABLE: '1' },
      }).toString('base64');
    },
    run(encoded, env = {}) {
      return spawnSync('python3', [script], { encoding: 'utf8', env: {
        ...process.env, CI: 'true', GITHUB_ACTIONS: 'true', OHOS_SIGNING_BUNDLE_BASE64: encoded, ...env,
      } });
    },
  };
}

test('standard tar bundle restores signing inputs with private modes and masks passwords', (t) => {
  const f = fixture(t);
  const result = f.run(f.archive());
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(readFileSync(path.join(f.app, 'signing.local.json'))), config);
  assert.equal(readFileSync(path.join(f.app, 'signing/release.p12'), 'utf8'), 'fixture: release.p12');
  assert.equal(statSync(path.join(f.app, 'signing')).mode & 0o777, 0o700);
  assert.equal(statSync(path.join(f.app, 'signing/release.p12')).mode & 0o777, 0o600);
  assert.match(result.stdout, new RegExp(`::add-mask::${password}`));
});

test('restore requires CI and refuses to overwrite local signing files', (t) => {
  const f = fixture(t);
  assert.notEqual(f.run(f.archive(), { CI: 'false' }).status, 0);
  writeFileSync(path.join(f.app, 'signing.local.json'), 'keep local configuration');
  assert.notEqual(f.run(f.archive()).status, 0);
  assert.equal(readFileSync(path.join(f.app, 'signing.local.json'), 'utf8'), 'keep local configuration');
  assert.equal(existsSync(path.join(f.app, 'signing')), false);
});

test('malformed configuration or missing material fails without leaving files or leaking values', (t) => {
  const f = fixture(t);
  for (const input of ['{"secret":"DO_NOT_LOG"', JSON.stringify(config)]) {
    writeFileSync(path.join(f.inputs, 'signing.local.json'), input);
    rmSync(path.join(f.inputs, 'signing/material'), { recursive: true, force: true });
    const result = f.run(f.archive());
    assert.notEqual(result.status, 0);
    assert.doesNotMatch(result.stdout + result.stderr, /DO_NOT_LOG|a1a1a1/);
    assert.equal(existsSync(path.join(f.app, 'signing')), false);
    assert.equal(existsSync(path.join(f.app, 'signing.local.json')), false);
  }
});

test('archive paths and links cannot write outside the signing inputs', (t) => {
  const f = fixture(t);
  for (const [name, type] of [['../escape', '0'], ['/tmp/escape', '0'], ['unrelated.txt', '0'], ['signing/link', '2'], ['signing/link', '1']]) {
    const encoded = execFileSync('python3', ['-c', `
import base64, io, sys, tarfile
buffer = io.BytesIO()
with tarfile.open(fileobj=buffer, mode='w:gz') as archive:
    item = tarfile.TarInfo(sys.argv[1])
    item.type = sys.argv[2].encode()
    item.linkname = '../../escape'
    archive.addfile(item)
print(base64.b64encode(buffer.getvalue()).decode())
`, name, type], { encoding: 'utf8' }).trim();
    const result = f.run(encoded);
    assert.notEqual(result.status, 0);
    assert.equal(existsSync(path.join(f.root, 'ohos/escape')), false);
    assert.equal(existsSync(path.join(f.app, 'signing')), false);
  }
});
