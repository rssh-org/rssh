import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { stageProject } from './stage-project.mjs';

test('staging updates source inputs, removes deleted inputs, and preserves generated state', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh staging '));
  const source = path.join(root, 'source');
  const destination = path.join(root, 'generated');
  const write = (base, name, content) => {
    const file = path.join(base, name);
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, content);
    return file;
  };
  try {
    write(source, 'src/main.txt', 'first');
    write(source, 'src/removed.txt', 'remove me');
    const executable = write(source, 'build.sh', '#!/bin/sh\n');
    chmodSync(executable, 0o755);
    write(source, 'build/cache', 'do not stage');
    write(source, 'signing.local.json', 'do not stage');
    write(destination, 'build/result', 'keep');
    write(destination, 'local.properties', 'keep local');
    const options = { exclude: (name) => name === 'build' || name === 'signing.local.json' };

    stageProject(source, destination, options);
    assert.equal(readFileSync(path.join(destination, 'src/main.txt'), 'utf8'), 'first');
    assert.equal(existsSync(path.join(destination, 'build/cache')), false);
    assert.equal(existsSync(path.join(destination, 'signing.local.json')), false);
    if (process.platform !== 'win32') assert.equal(statSync(path.join(destination, 'build.sh')).mode & 0o777, 0o755);
    const before = statSync(path.join(destination, 'src/main.txt')).mtimeMs;
    stageProject(source, destination, options);
    assert.equal(statSync(path.join(destination, 'src/main.txt')).mtimeMs, before);

    write(source, 'src/main.txt', 'second');
    rmSync(path.join(source, 'src/removed.txt'));
    stageProject(source, destination, options);
    assert.equal(readFileSync(path.join(destination, 'src/main.txt'), 'utf8'), 'second');
    assert.equal(existsSync(path.join(destination, 'src/removed.txt')), false);
    assert.equal(readFileSync(path.join(destination, 'build/result'), 'utf8'), 'keep');
    assert.equal(readFileSync(path.join(destination, 'local.properties'), 'utf8'), 'keep local');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('a missing source fails without deleting the previously prepared project', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh staging missing '));
  try {
    const destination = path.join(root, 'generated');
    mkdirSync(destination);
    writeFileSync(path.join(destination, 'input'), 'keep');
    writeFileSync(path.join(destination, '.rssh-source-files.json'), '["input"]\n');
    assert.throws(() => stageProject(path.join(root, 'missing'), destination), /ENOENT/);
    assert.equal(readFileSync(path.join(destination, 'input'), 'utf8'), 'keep');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('staging handles file and directory transitions while preserving unowned outputs', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh staging transitions '));
  const source = path.join(root, 'source');
  const destination = path.join(root, 'generated');
  const managed = 'src/feature';
  try {
    mkdirSync(path.join(source, 'src'), { recursive: true });
    writeFileSync(path.join(source, managed), 'single file');
    stageProject(source, destination);
    mkdirSync(path.join(destination, 'src/cache'));
    writeFileSync(path.join(destination, 'src/cache/output'), 'keep generated output');

    rmSync(path.join(source, managed));
    mkdirSync(path.join(source, managed));
    writeFileSync(path.join(source, managed, 'module'), 'nested file');
    stageProject(source, destination);
    assert.equal(readFileSync(path.join(destination, managed, 'module'), 'utf8'), 'nested file');
    assert.equal(readFileSync(path.join(destination, 'src/cache/output'), 'utf8'), 'keep generated output');

    rmSync(path.join(source, managed), { recursive: true });
    writeFileSync(path.join(source, managed), 'single file again');
    stageProject(source, destination);
    assert.equal(readFileSync(path.join(destination, managed), 'utf8'), 'single file again');
    assert.equal(readFileSync(path.join(destination, 'src/cache/output'), 'utf8'), 'keep generated output');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('staging never removes unowned files to replace a directory with a source file', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh staging unowned '));
  const source = path.join(root, 'source');
  const destination = path.join(root, 'generated');
  try {
    mkdirSync(path.join(source, 'feature'), { recursive: true });
    writeFileSync(path.join(source, 'feature/module'), 'managed source');
    stageProject(source, destination);
    writeFileSync(path.join(destination, 'feature/cache'), 'unowned output');

    rmSync(path.join(source, 'feature'), { recursive: true });
    writeFileSync(path.join(source, 'feature'), 'replacement source');
    assert.throws(() => stageProject(source, destination), /Cannot replace a generated directory/);
    assert.equal(readFileSync(path.join(destination, 'feature/cache'), 'utf8'), 'unowned output');
    assert.equal(existsSync(path.join(destination, 'feature/module')), false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('stale-input cleanup cannot escape the generated project', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh staging manifest '));
  try {
    const source = path.join(root, 'source');
    const destination = path.join(root, 'generated');
    mkdirSync(source);
    mkdirSync(destination);
    writeFileSync(path.join(root, 'outside'), 'keep');
    writeFileSync(path.join(destination, '.rssh-source-files.json'), '["../outside"]\n');
    assert.throws(() => stageProject(source, destination), /Invalid staged path/);
    assert.equal(readFileSync(path.join(root, 'outside'), 'utf8'), 'keep');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
