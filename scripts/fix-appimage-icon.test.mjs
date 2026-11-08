import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readlinkSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const script = fileURLToPath(new URL('../.github/scripts/fix-appimage-icon.sh', import.meta.url));
const realpathProbe = spawnSync('realpath', ['-ms', '.'], { encoding: 'utf8' });
const skip = process.platform === 'win32' || (process.platform !== 'linux' && realpathProbe.status !== 0)
  ? 'requires Bash and GNU realpath with -ms support'
  : false;

function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh appimage '));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const source = path.join(root, 'source');
  const packed = path.join(root, 'packed');
  const bundle = path.join(root, 'bundle');
  const bin = path.join(root, 'bin');
  for (const directory of [source, bundle, bin]) mkdirSync(directory);
  const executable = (name, body) => {
    writeFileSync(name, `#!${process.execPath}\n${body}\n`);
    chmodSync(name, 0o755);
  };
  const image = path.join(bundle, 'RSSH.AppImage');
  executable(image, `
const fs = require('node:fs');
const path = require('node:path');
if (process.argv[2] !== '--appimage-extract') process.exit(90);
const count = Number(fs.existsSync(process.env.RSSH_TEST_EXTRACT_COUNT)
  ? fs.readFileSync(process.env.RSSH_TEST_EXTRACT_COUNT, 'utf8') : 0);
fs.writeFileSync(process.env.RSSH_TEST_EXTRACT_COUNT, String(count + 1));
fs.cpSync(count === 0 ? process.env.RSSH_TEST_SOURCE : process.env.RSSH_TEST_PACKED,
  path.join(process.cwd(), 'squashfs-root'), { recursive: true, verbatimSymlinks: true });
`);
  const packer = path.join(bin, 'appimagetool');
  executable(packer, `
const fs = require('node:fs');
const path = require('node:path');
if (process.argv[2] !== '--appimage-extract-and-run') process.exit(91);
fs.writeFileSync(process.env.RSSH_TEST_PACK_COUNT, '1');
if (process.env.RSSH_TEST_PACK_FAIL) process.exit(58);
fs.cpSync(process.argv[3], process.env.RSSH_TEST_PACKED,
  { recursive: true, verbatimSymlinks: true });
if (process.env.RSSH_TEST_PACK_ESCAPE) {
  fs.symlinkSync('../outside.dat', path.join(process.env.RSSH_TEST_PACKED, 'resource'));
}
`);
  // An invalid mock configuration must fail without downloading anything.
  executable(path.join(bin, 'curl'), 'process.exit(99);');
  const env = {
    ...process.env,
    PATH: `${bin}${path.delimiter}${process.env.PATH}`,
    APPIMAGETOOL: packer,
    RSSH_TEST_SOURCE: source,
    RSSH_TEST_PACKED: packed,
    RSSH_TEST_EXTRACT_COUNT: path.join(root, 'extractions'),
    RSSH_TEST_PACK_COUNT: path.join(root, 'packs'),
  };
  return {
    source, packed, bundle, env,
    file(name, contents = 'icon') {
      const destination = path.join(source, name);
      mkdirSync(path.dirname(destination), { recursive: true });
      writeFileSync(destination, contents);
    },
    link(name, target) {
      const destination = path.join(source, name);
      mkdirSync(path.dirname(destination), { recursive: true });
      symlinkSync(target, destination);
    },
    failCommand(command, extraction) {
      const lookup = spawnSync('bash', ['-c', 'command -v "$1"', '--', command], { encoding: 'utf8' });
      assert.equal(lookup.status, 0, `cannot locate ${command}`);
      executable(path.join(bin, command), `
const fs = require('node:fs');
const { spawnSync } = require('node:child_process');
if (Number(fs.readFileSync(process.env.RSSH_TEST_EXTRACT_COUNT, 'utf8')) === ${extraction}) {
  console.error('injected ${command} failure');
  process.exit(57);
}
const result = spawnSync(${JSON.stringify(lookup.stdout.trim())}, process.argv.slice(2), { stdio: 'inherit' });
process.exit(result.status ?? 1);
`);
    },
    run() {
      return spawnSync('bash', [script, image, 'x86_64'], { env, encoding: 'utf8', timeout: 10_000 });
    },
  };
}

function assertFailure(result) {
  assert.ifError(result.error);
  assert.notEqual(result.status, 0, result.stdout + result.stderr);
  assert.doesNotMatch(result.stdout, /^Fixed /m);
}

test('repairs the absolute icon and makes the AppDir root readable before packing', { skip }, (t) => {
  const f = fixture(t);
  f.file('RSSH.png');
  f.link('.DirIcon', '/home/build/RSSH.AppDir/RSSH.png');
  chmodSync(f.source, 0o700);
  const result = f.run();
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readlinkSync(path.join(f.packed, '.DirIcon')), 'RSSH.png');
  assert.equal(statSync(path.join(f.packed, '.DirIcon')).isFile(), true);
  assert.equal(statSync(f.packed).mode & 0o777, 0o755);
  assert.equal(readFileSync(f.env.RSSH_TEST_EXTRACT_COUNT, 'utf8'), '2');
  assert.match(result.stdout, /^Fixed RSSH.AppImage$/m);
});

test('preserves valid and dangling relative links inside the image', { skip }, (t) => {
  const f = fixture(t);
  f.file('RSSH.png');
  f.link('.DirIcon', 'RSSH.png');
  f.link('usr/optional', '../missing');
  const result = f.run();
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readlinkSync(path.join(f.packed, '.DirIcon')), 'RSSH.png');
  assert.equal(readlinkSync(path.join(f.packed, 'usr/optional')), '../missing');
  assert.match(result.stderr, /WARNING:.*optional/);
});

for (const target of ['/.DirIcon', '.DirIcon']) {
  test(`rejects self-reference ${target}`, { skip }, (t) => {
    const f = fixture(t);
    f.link('.DirIcon', target);
    assertFailure(f.run());
  });
}

for (const target of ['../outside.dat', '../squashfs-root-extra/outside.dat']) {
  for (const present of [true, false]) {
    test(`rejects ${present ? 'existing' : 'missing'} relative escape ${target}`, { skip }, (t) => {
      const f = fixture(t);
      f.link('escape', target);
      const outside = path.resolve(f.bundle, 'squashfs-root', target);
      if (present) {
        mkdirSync(path.dirname(outside), { recursive: true });
        writeFileSync(outside, 'outside');
      }
      assertFailure(f.run());
      if (present) assert.equal(readFileSync(outside, 'utf8'), 'outside');
    });
  }
}

test('rejects a nested symlink chain escaping the image', { skip }, (t) => {
  const f = fixture(t);
  f.link('sub/alias', '../../outside.dat');
  f.link('escape', 'sub/alias');
  writeFileSync(path.join(f.bundle, 'outside.dat'), 'outside');
  assertFailure(f.run());
  assert.equal(readFileSync(path.join(f.bundle, 'outside.dat'), 'utf8'), 'outside');
});

test('rejects traversal through an internal directory symlink', { skip }, (t) => {
  const f = fixture(t);
  f.link('sub/pivot', '..');
  f.link('escape', 'sub/pivot/../outside.dat');
  writeFileSync(path.join(f.bundle, 'outside.dat'), 'outside');
  assertFailure(f.run());
  assert.equal(readFileSync(path.join(f.bundle, 'outside.dat'), 'utf8'), 'outside');
});

for (const command of ['find', 'readlink', 'realpath']) {
  for (const extraction of [1, 2]) {
    test(`propagates ${command} failure during ${extraction === 1 ? 'initial' : 'post-repack'} scan`, { skip }, (t) => {
      const f = fixture(t);
      f.file('RSSH.png');
      f.link('.DirIcon', 'RSSH.png');
      f.failCommand(command, extraction);
      const result = f.run();
      assertFailure(result);
      assert.match(result.stderr, new RegExp(`injected ${command} failure`));
      assert.equal(existsSync(f.env.RSSH_TEST_PACK_COUNT), extraction === 2);
    });
  }
}

test('propagates appimagetool failure', { skip }, (t) => {
  const f = fixture(t);
  f.file('RSSH.png');
  f.env.RSSH_TEST_PACK_FAIL = '1';
  const result = f.run();
  assertFailure(result);
  assert.equal(result.status, 58);
  assert.equal(readFileSync(f.env.RSSH_TEST_EXTRACT_COUNT, 'utf8'), '1');
});

test('rejects an escaping resource introduced by repacking', { skip }, (t) => {
  const f = fixture(t);
  f.file('RSSH.png');
  f.link('.DirIcon', 'RSSH.png');
  writeFileSync(path.join(f.bundle, 'outside.dat'), 'outside');
  f.env.RSSH_TEST_PACK_ESCAPE = '1';
  assertFailure(f.run());
  assert.equal(readFileSync(f.env.RSSH_TEST_EXTRACT_COUNT, 'utf8'), '2');
  assert.equal(readlinkSync(path.join(f.packed, 'resource')), '../outside.dat');
  assert.equal(readFileSync(path.join(f.bundle, 'outside.dat'), 'utf8'), 'outside');
});
