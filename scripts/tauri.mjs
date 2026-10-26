import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { prepareAndroid } from '../android/scripts/prepare.mjs';
import { prepareDesktop } from '../desktop/scripts/prepare-desktop.mjs';

export function runTauri(root, args, { stdio = 'inherit', env = process.env } = {}) {
  const informational = args.some((arg) => ['--help', '-h', '--version', '-V'].includes(arg));
  const command = args.find((arg) => !arg.startsWith('-'));
  if (command === 'android' && !informational) prepareAndroid(root);
  const packaging = command === 'build' && !informational ? prepareDesktop(root, args) : [];
  const cli = path.join(root, 'node_modules/@tauri-apps/cli/tauri.js');
  const separator = args.indexOf('--');
  const cliArgs = separator < 0 ? [...args, ...packaging] : [...args.slice(0, separator), ...packaging, ...args.slice(separator)];
  const result = spawnSync(process.execPath, [cli, ...cliArgs], {
    cwd: root,
    env: { ...env, TAURI_APP_PATH: path.join(root, 'app'), TAURI_FRONTEND_PATH: root },
    stdio, encoding: 'utf8',
  });
  if (result.error) throw result.error;
  return result;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)));
    const result = runTauri(root, process.argv.slice(2));
    process.exitCode = result.status ?? 1;
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
