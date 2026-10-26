import { chmodSync, copyFileSync, existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, renameSync, rmdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';

const manifestName = '.rssh-source-files.json';

function validatePath(name) {
  if (typeof name !== 'string' || !name || name.includes('\\') || name.split('/').some((part) => !part || part === '.' || part === '..') || path.isAbsolute(name) || name === manifestName) {
    throw new Error(`Invalid staged path: ${name}`);
  }
}

function collectFiles(source, exclude, prefix = '') {
  return readdirSync(path.join(source, prefix), { withFileTypes: true }).flatMap((entry) => {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (relative === manifestName || exclude(relative, entry)) return [];
    if (entry.isDirectory()) return collectFiles(source, exclude, relative);
    if (!entry.isFile()) throw new Error(`Project source must be a regular file: ${relative}`);
    validatePath(relative);
    return [relative];
  }).sort();
}

function destinationFile(destination, relative) {
  let current = destination;
  for (const part of relative.split('/')) {
    current = path.join(current, part);
    if (existsSync(current) && lstatSync(current).isSymbolicLink()) {
      throw new Error(`Generated project contains a symlink: ${current}`);
    }
  }
  return current;
}

function pruneEmptyParents(file, destination) {
  const root = path.resolve(destination);
  for (let directory = path.dirname(path.resolve(file)); directory !== root; directory = path.dirname(directory)) {
    try {
      rmdirSync(directory);
    } catch (error) {
      if (error.code === 'ENOENT') continue;
      if (error.code === 'ENOTEMPTY' || error.code === 'EEXIST') return;
      throw error;
    }
  }
}

// Only paths recorded by this function belong to source synchronization. Gradle,
// Hvigor, SDK settings and other generated files remain owned by their producers.
export function stageProject(source, destination, { exclude = () => false } = {}) {
  const files = collectFiles(source, exclude);
  const manifest = path.join(destination, manifestName);
  const previous = existsSync(manifest) ? JSON.parse(readFileSync(manifest, 'utf8')) : [];
  if (!Array.isArray(previous)) throw new Error('Invalid staged source manifest');
  previous.forEach(validatePath);

  // Remove old managed paths before copying so a file can become a directory
  // (or vice versa). Prune only empty parents; never remove generated contents.
  const current = new Set(files);
  for (const relative of previous) {
    if (current.has(relative)) continue;
    const file = destinationFile(destination, relative);
    rmSync(file, { force: true });
    pruneEmptyParents(file, destination);
  }

  for (const relative of files) {
    const from = path.join(source, relative);
    const to = destinationFile(destination, relative);
    const mode = statSync(from).mode & 0o777;
    if (existsSync(to) && !statSync(to).isFile()) {
      throw new Error(`Cannot replace a generated directory with a source file: ${to}`);
    }
    if (existsSync(to) && readFileSync(from).equals(readFileSync(to))) {
      if ((statSync(to).mode & 0o777) !== mode) chmodSync(to, mode);
      continue;
    }
    mkdirSync(path.dirname(to), { recursive: true });
    const temporary = `${to}.rssh-${process.pid}`;
    try {
      copyFileSync(from, temporary);
      chmodSync(temporary, mode);
      renameSync(temporary, to);
    } finally {
      rmSync(temporary, { force: true });
    }
  }

  const content = JSON.stringify(files, null, 2) + '\n';
  if (!existsSync(manifest) || readFileSync(manifest, 'utf8') !== content) {
    mkdirSync(destination, { recursive: true });
    const temporary = `${manifest}.${process.pid}`;
    try {
      writeFileSync(temporary, content);
      renameSync(temporary, manifest);
    } finally {
      rmSync(temporary, { force: true });
    }
  }
  return files;
}
