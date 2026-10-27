import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { gzipSync } from 'node:zlib';
import { cleanSigning, packSigning, restoreSigning } from './ci-signing.mjs';

const password = 'a1'.repeat(32);
const config = () => ({ signingConfigs: [{ name: 'default', type: 'HarmonyOS', material: {
  storeFile: 'signing/release.p12', storePassword: password, keyAlias: 'fixture',
  keyPassword: 'b2'.repeat(32), signAlg: 'SHA256withECDSA',
  profile: 'signing/release.p7b', certpath: 'signing/release.cer',
} }] });

function fixture(t, withSigning = true) {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh ci signing '));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const app = path.join(root, 'ohos/app');
  mkdirSync(app, { recursive: true });
  const write = (file, value) => {
    const target = path.join(app, file);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, value);
  };
  if (withSigning) {
    write('signing.local.json', JSON.stringify(config()));
    for (const name of ['release.p12', 'release.p7b', 'release.cer', 'material/fd/one/key', 'material/ac/salt', 'material/ce/key']) {
      write(`signing/${name}`, `test-only fixture: ${name}`);
    }
  }
  return { root, app, write };
}

function bundle(t) {
  const input = fixture(t);
  const output = path.join(input.root, 'bundle.base64');
  packSigning(input.root, output);
  return readFileSync(output, 'utf8');
}

// Small USTAR fixtures exercise the archive boundary without invoking platform-specific tar transforms.
function archive(entries) {
  const chunks = [];
  for (const { name, type = '0', data = '' } of entries) {
    const bytes = Buffer.from(data);
    const header = Buffer.alloc(512);
    header.write(name, 0, 100);
    header.write('0000600\0', 100);
    header.write('0000000\0', 108);
    header.write('0000000\0', 116);
    header.write(`${bytes.length.toString(8).padStart(11, '0')}\0`, 124);
    header.write('00000000000\0', 136);
    header.fill(32, 148, 156);
    header.write(type, 156);
    header.write('ustar\0', 257);
    header.write('00', 263);
    const sum = header.reduce((total, byte) => total + byte, 0);
    header.write(`${sum.toString(8).padStart(6, '0')}\0 `, 148);
    chunks.push(header, bytes, Buffer.alloc((512 - bytes.length % 512) % 512));
  }
  chunks.push(Buffer.alloc(1024));
  return gzipSync(Buffer.concat(chunks)).toString('base64');
}

test('pack and restore preserve only signing inputs with private permissions; clean also removes staged copies', (t) => {
  const input = fixture(t);
  input.write('unrelated.txt', 'leave alone');
  const output = path.join(input.root, 'bundle.base64');
  packSigning(input.root, output);
  assert.equal(statSync(output).mode & 0o777, 0o600);
  const encoded = readFileSync(output, 'utf8');
  assert.ok(encoded.length <= 48 * 1024);
  assert.throws(() => packSigning(input.root, output), /already exists/);
  const target = fixture(t, false);
  restoreSigning(target.root, { CI: 'true', OHOS_SIGNING_BUNDLE_BASE64: encoded });
  assert.deepEqual(JSON.parse(readFileSync(path.join(target.app, 'signing.local.json'))), config());
  assert.equal(existsSync(path.join(target.app, 'unrelated.txt')), false);
  assert.equal(statSync(path.join(target.app, 'signing')).mode & 0o777, 0o700);
  assert.equal(statSync(path.join(target.app, 'signing/release.p12')).mode & 0o777, 0o600);
  assert.equal(statSync(path.join(target.app, 'signing/material')).mode & 0o777, 0o700);
  const generated = path.join(target.root, 'app/gen/ohos');
  mkdirSync(generated, { recursive: true });
  cpSync(path.join(target.app, 'signing'), path.join(generated, 'signing'), { recursive: true });
  cpSync(path.join(target.app, 'signing.local.json'), path.join(generated, 'signing.local.json'));
  writeFileSync(path.join(generated, 'keep.txt'), 'keep');
  cleanSigning(target.root, { CI: 'true' });
  cleanSigning(target.root, { CI: 'true' });
  for (const base of [target.app, generated]) {
    assert.equal(existsSync(path.join(base, 'signing')), false);
    assert.equal(existsSync(path.join(base, 'signing.local.json')), false);
  }
  assert.equal(readFileSync(path.join(generated, 'keep.txt'), 'utf8'), 'keep');
  assert.ok(existsSync(path.join(input.app, 'signing/release.p12')));
});

test('restore and clean require CI=true and never replace existing local signing inputs', (t) => {
  const input = fixture(t);
  const encoded = bundle(t);
  for (const env of [{}, { CI: 'false' }]) {
    assert.throws(() => restoreSigning(input.root, { ...env, OHOS_SIGNING_BUNDLE_BASE64: encoded }), /CI=true/);
    assert.throws(() => cleanSigning(input.root, env), /CI=true/);
  }
  assert.throws(() => restoreSigning(input.root, { CI: 'true', OHOS_SIGNING_BUNDLE_BASE64: encoded }), /already exist/);
  assert.throws(() => cleanSigning(input.root, { CI: 'true' }), /not restored by this tool/);
  assert.deepEqual(JSON.parse(readFileSync(path.join(input.app, 'signing.local.json'))), config());
});

test('configuration failures do not reveal passwords or JSON fragments', (t) => {
  const input = fixture(t);
  const output = path.join(input.root, 'bundle.base64');
  input.write('signing.local.json', '{"password":"NEVER_PRINT_THIS"');
  assert.throws(() => packSigning(input.root, output), (error) => /valid JSON/.test(error.message) && !error.message.includes('NEVER_PRINT_THIS'));
  for (const bad of ['../outside.p12', '/tmp/key.p12', 'signing/../outside.p12', 'signing\\key.p12']) {
    const value = config(); value.signingConfigs[0].material.storeFile = bad;
    input.write('signing.local.json', JSON.stringify(value));
    assert.throws(() => packSigning(input.root, output), /relative signing\//);
  }
  const value = config(); value.signingConfigs[0].material.storePassword = 'plain-password';
  input.write('signing.local.json', JSON.stringify(value));
  assert.throws(() => packSigning(input.root, output), /DevEco encrypted/);
});

test('pack rejects missing material and links instead of reading their targets', (t) => {
  const input = fixture(t);
  const output = path.join(input.root, 'bundle.base64');
  rmSync(path.join(input.app, 'signing/material'), { recursive: true });
  assert.throws(() => packSigning(input.root, output), /material directory/);
  input.write('signing/material/fixture', 'fixture');
  symlinkSync('/does-not-exist', path.join(input.app, 'signing/link'));
  assert.throws(() => packSigning(input.root, output), /links or special files/);
});

test('restore rejects path escapes, links, metadata extensions and duplicate paths before writing', (t) => {
  const target = fixture(t, false);
  const attacks = [
    [{ name: '../escape' }], [{ name: '/tmp/escape' }], [{ name: 'signing/../../escape' }],
    [{ name: 'unrelated.txt' }], [{ name: 'signing/link', type: '2' }],
    [{ name: 'signing/link', type: '1' }], [{ name: 'pax', type: 'x' }],
    [{ name: 'signing/file' }, { name: 'signing/file' }],
  ];
  for (const entries of attacks) {
    assert.throws(() => restoreSigning(target.root, { CI: 'true', OHOS_SIGNING_BUNDLE_BASE64: archive(entries) }), /archive/);
    assert.equal(existsSync(path.join(target.app, 'signing')), false);
    assert.equal(existsSync(path.join(target.app, 'signing.local.json')), false);
  }
});

test('both directions enforce the 48 KiB GitHub secret limit and restore bounds decompression', (t) => {
  const input = fixture(t);
  input.write('signing/release.p12', randomBytes(40 * 1024));
  const output = path.join(input.root, 'bundle.base64');
  assert.throws(() => packSigning(input.root, output), /48 KiB/);
  assert.equal(existsSync(output), false);
  const target = fixture(t, false);
  assert.throws(() => restoreSigning(target.root, { CI: 'true', OHOS_SIGNING_BUNDLE_BASE64: 'a'.repeat(48 * 1024 + 1) }), /48 KiB/);
  assert.throws(() => restoreSigning(target.root, { CI: 'true', OHOS_SIGNING_BUNDLE_BASE64: 'not base64!' }), /base64/);
  const oversized = gzipSync(Buffer.alloc(8 * 1024 * 1024 + 512)).toString('base64');
  assert.throws(() => restoreSigning(target.root, { CI: 'true', OHOS_SIGNING_BUNDLE_BASE64: oversized }), /gzip|8 MiB/);
});

test('GitHub restore masks both passwords; local restores print no signing values', (t) => {
  const encoded = bundle(t);
  const target = fixture(t, false);
  const lines = [];
  restoreSigning(target.root, { CI: 'true', GITHUB_ACTIONS: 'true', OHOS_SIGNING_BUNDLE_BASE64: encoded }, (line) => lines.push(line));
  assert.deepEqual(lines, [`::add-mask::${password}`, `::add-mask::${'b2'.repeat(32)}`]);
  const other = fixture(t, false);
  restoreSigning(other.root, { CI: 'true', OHOS_SIGNING_BUNDLE_BASE64: encoded }, () => assert.fail('Unexpected signing output'));
});

test('clean refuses modified inputs and leaves every candidate untouched', (t) => {
  const target = fixture(t, false);
  restoreSigning(target.root, { CI: 'true', OHOS_SIGNING_BUNDLE_BASE64: bundle(t) });
  target.write('signing/new-file', 'not owned by restore');
  assert.throws(() => cleanSigning(target.root, { CI: 'true' }), /changed since restore/);
  assert.equal(readFileSync(path.join(target.app, 'signing/new-file'), 'utf8'), 'not owned by restore');
  assert.ok(existsSync(path.join(target.app, 'signing.local.json')));
});

test('clean only owns the restoring GitHub run and job', (t) => {
  const target = fixture(t, false);
  const env = { CI: 'true', GITHUB_RUN_ID: '100', GITHUB_RUN_ATTEMPT: '1', GITHUB_JOB: 'ohos' };
  restoreSigning(target.root, { ...env, OHOS_SIGNING_BUNDLE_BASE64: bundle(t) });
  assert.throws(() => cleanSigning(target.root, { ...env, GITHUB_RUN_ID: '101' }), /another CI run/);
  assert.ok(existsSync(path.join(target.app, 'signing.local.json')));
  cleanSigning(target.root, env);
  assert.equal(existsSync(path.join(target.app, 'signing')), false);
});

test('pack also counts tar headers toward the restore unpacked-size limit', (t) => {
  const input = fixture(t);
  input.write('signing/release.p12', Buffer.alloc(8 * 1024 * 1024 - 2048));
  const output = path.join(input.root, 'bundle.base64');
  assert.throws(() => packSigning(input.root, output), /8 MiB/);
  assert.equal(existsSync(output), false);
});
