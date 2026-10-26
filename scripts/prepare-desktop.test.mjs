import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { prepareDesktop } from './prepare-desktop.mjs';

function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh desktop '));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(path.join(root, 'src-tauri'));
  return root;
}

test('desktop packaging uses Cargo executable output and maps only the staged CLI', (t) => {
  const root = fixture(t);
  const artifact = path.join(root, 'custom-target', 'rssh-cli.exe');
  mkdirSync(path.dirname(artifact));
  writeFileSync(artifact, 'compiled CLI');
  const calls = [];
  const args = prepareDesktop(root, ['build', '--target=x86_64-pc-windows-msvc', '--debug'], {
    run(command, argv, options) {
      calls.push({ command, argv, cwd: options.cwd });
      return { status: 0, stdout: JSON.stringify({ reason: 'compiler-artifact', target: { name: 'rssh-cli' }, executable: artifact }) + '\n' };
    },
  });
  assert.equal(calls[0].command, 'cargo');
  assert.equal(calls[0].cwd, path.join(root, 'src-tauri'));
  assert.ok(calls[0].argv.includes('x86_64-pc-windows-msvc'));
  assert.ok(!calls[0].argv.includes('--release'));
  assert.equal(readFileSync(path.join(root, 'src-tauri/bin/rssh-cli.exe'), 'utf8'), 'compiled CLI');
  assert.deepEqual(args, ['--config', JSON.stringify({ bundle: { resources: { 'bin/rssh-cli.exe': 'bin/rssh-cli.exe' } } })]);
});

test('failed or missing CLI artifacts never produce bundle configuration', (t) => {
  const root = fixture(t);
  assert.throws(() => prepareDesktop(root, ['build'], { run: () => ({ status: 1, stdout: '' }) }), /CLI build failed/);
  assert.throws(() => prepareDesktop(root, ['build'], { run: () => ({ status: 0, stdout: '' }) }), /executable/);
  assert.equal(existsSync(path.join(root, 'src-tauri/bin')), false);
});

test('help and unbundled builds do not require or create a CLI', (t) => {
  const root = fixture(t);
  for (const args of [['build', '--help'], ['build', '-h'], ['build', '--no-bundle']]) {
    assert.deepEqual(prepareDesktop(root, args, { run() { assert.fail('must not build'); } }), []);
  }
  assert.equal(existsSync(path.join(root, 'src-tauri/bin')), false);
});

test('attached short target options select the CLI target too', (t) => {
  const root = fixture(t);
  const executable = path.join(root, 'rssh-cli');
  writeFileSync(executable, 'cross-compiled CLI');
  for (const option of ['-tx86_64-apple-darwin', '-t=x86_64-apple-darwin']) {
    prepareDesktop(root, ['build', option], {
      run(command, argv) {
        assert.equal(argv[argv.indexOf('--target') + 1], 'x86_64-apple-darwin');
        return { status: 0, stdout: JSON.stringify({ reason: 'compiler-artifact', target: { name: 'rssh-cli' }, executable }) };
      },
    });
  }
});

test('universal macOS packaging builds and combines both CLI architectures', (t) => {
  const root = fixture(t);
  const calls = [];
  prepareDesktop(root, ['build', '--target', 'universal-apple-darwin'], {
    run(command, argv) {
      calls.push([command, ...argv]);
      if (command === 'lipo') {
        writeFileSync(argv[argv.indexOf('-output') + 1], 'universal CLI');
        return { status: 0 };
      }
      const target = argv[argv.indexOf('--target') + 1];
      const executable = path.join(root, target);
      writeFileSync(executable, target);
      return { status: 0, stdout: JSON.stringify({ reason: 'compiler-artifact', target: { name: 'rssh-cli' }, executable }) };
    },
  });
  assert.equal(calls.length, 3);
  assert.ok(calls[0].includes('--release'));
  assert.ok(calls[0].includes('aarch64-apple-darwin'));
  assert.ok(calls[1].includes('x86_64-apple-darwin'));
  assert.equal(readFileSync(path.join(root, 'src-tauri/bin/rssh-cli'), 'utf8'), 'universal CLI');
});
