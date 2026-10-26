import { cpSync, existsSync, rmSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { stageProject } from '../../scripts/stage-project.mjs';

const sourceDirectories = new Set(['AppScope', 'common', 'products', 'hvigor']);
const sourceFiles = new Set(['.gitignore', 'build-profile.json5', 'code-linter.json5', 'hvigorfile.ts', 'oh-package.json5', 'oh-package-lock.json5', 'signing.example.json']);
const generatedDirectories = new Set(['build', 'oh_modules', 'node_modules', '.hvigor', '.idea', '.cxx', '.test']);

export function prepareProject(root) {
  const source = path.join(root, 'ohos/app');
  const destination = path.join(root, 'app/gen/ohos');
  stageProject(source, destination, { exclude: (relative, entry) => {
    if (!relative.includes('/')) return !sourceDirectories.has(relative) && !sourceFiles.has(relative);
    if (entry.isDirectory() && generatedDirectories.has(entry.name)) return true;
    return relative === 'common/runtime/libs'
      || relative === 'common/runtime/src/main/resources/rawfile'
      || relative === 'common/runtime/BuildProfile.ets'
      || entry.name === '.DS_Store';
  } });

  // These are explicit local inputs, never part of the source manifest or Git.
  // Keep their relative layout so Hvigor sees the same signing and SDK paths.
  for (const name of ['local.properties', 'signing.local.json', 'signing']) {
    const from = path.join(source, name);
    const to = path.join(destination, name);
    rmSync(to, { recursive: true, force: true });
    if (existsSync(from)) cpSync(from, to, { recursive: true });
  }
  return destination;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = fileURLToPath(new URL('../..', import.meta.url));
  process.stdout.write(prepareProject(root) + '\n');
}
