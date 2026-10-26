import { spawnSync } from 'node:child_process';
import { chmodSync, copyFileSync, mkdirSync, statSync } from 'node:fs';
import path from 'node:path';

function targetArgument(args) {
  for (let index = 0; index < args.length; index++) {
    if (args[index].startsWith('--target=')) return args[index].slice('--target='.length);
    if (args[index].startsWith('-t') && args[index].length > 2) return args[index].slice(2).replace(/^=/, '');
    if (args[index] === '--target' || args[index] === '-t') {
      if (!args[index + 1] || args[index + 1].startsWith('-')) throw new Error('Missing --target value');
      return args[index + 1];
    }
  }
}

function buildCli(root, target, debug, run) {
  const args = ['build', '--package', 'rssh-cli', '--bin', 'rssh-cli', '--message-format=json-render-diagnostics'];
  if (!debug) args.push('--release');
  if (target) args.push('--target', target);
  const result = run('cargo', args, {
    cwd: root, encoding: 'utf8',
    stdio: ['inherit', 'pipe', 'inherit'], maxBuffer: 64 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`CLI build failed${target ? ` for ${target}` : ''}`);
  // Cargo reports the actual executable, including configured target directories.
  // Do not reconstruct it from target/profile assumptions.
  for (const line of result.stdout.trim().split('\n').reverse()) {
    if (!line) continue;
    const message = JSON.parse(line);
    if (message.reason === 'compiler-artifact' && message.target.name === 'rssh-cli' && message.executable) {
      return message.executable;
    }
  }
  throw new Error('Cargo produced no rssh-cli executable');
}

export function prepareDesktop(root, args, { run = spawnSync } = {}) {
  if (args.some((arg) => ['--help', '-h', '--version', '-V', '--no-bundle'].includes(arg))) return [];
  const target = targetArgument(args);
  const debug = args.includes('--debug') || args.includes('-d');
  const universal = target === 'universal-apple-darwin';
  const targets = universal ? ['aarch64-apple-darwin', 'x86_64-apple-darwin'] : [target];
  const executables = targets.map((triple) => buildCli(root, triple, debug, run));
  const name = universal ? 'rssh-cli' : path.basename(executables[0]);
  if (!['rssh-cli', 'rssh-cli.exe'].includes(name)) {
    throw new Error(`Unexpected CLI executable name: ${name}`);
  }
  const bin = path.join(root, 'app/bin');
  mkdirSync(bin, { recursive: true });
  const destination = path.join(bin, name);
  if (universal) {
    const result = run('lipo', ['-create', ...executables, '-output', destination], { stdio: 'inherit' });
    if (result.error) throw result.error;
    if (result.status !== 0) throw new Error('Failed to combine universal CLI');
  } else {
    copyFileSync(executables[0], destination);
    chmodSync(destination, statSync(executables[0]).mode);
  }
  // Only desktop packaging supplies this resource. Cargo tests, development,
  // headless and mobile builds have no dependency on a pre-existing bin/.
  return ['--config', JSON.stringify({ bundle: { resources: { [`bin/${name}`]: `bin/${name}` } } })];
}
