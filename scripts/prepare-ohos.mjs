import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// Keep the normal manifest as the only list of shared dependencies. The OHOS
// graph is a separate workspace because Cargo patches apply to an entire graph,
// including host build-dependencies; they cannot be scoped to a target cfg.
function pinVersion(manifest, section, name, version) {
  let currentSection;
  let matches = 0;
  const result = manifest.split('\n').map((line) => {
    if (line.startsWith('[')) currentSection = line.trim();
    if (currentSection !== `[${section}]` || !line.startsWith(`${name} = `)) return line;
    const replaced = line.replace(/version = "[^"]+"/, `version = "${version}"`);
    if (replaced === line) throw new Error(`Cannot pin ${section}.${name}: expected inline version`);
    matches++;
    return replaced;
  }).join('\n');
  if (matches !== 1) throw new Error(`Expected one ${section}.${name}, found ${matches}`);
  return result;
}

export function prepareWorkspace(root, workspace) {
  const source = path.join(root, 'src-tauri');
  let manifest = readFileSync(path.join(source, 'Cargo.toml'), 'utf8');
  manifest = pinVersion(manifest, 'dependencies', 'tauri', '=2.11.5');
  manifest = pinVersion(manifest, 'build-dependencies', 'tauri-build', '=2.6.3');
  const sourceRoot = path.join(source, 'target/ohos-sources');
  manifest += '\n' + readFileSync(path.join(source, 'ohos/overlay.toml'), 'utf8')
    .replaceAll('@OHOS_SOURCES@', JSON.stringify(sourceRoot).slice(1, -1));
  mkdirSync(workspace, { recursive: true });
  writeFileSync(path.join(workspace, 'Cargo.toml'), manifest);
  copyFileSync(path.join(source, 'ohos/Cargo.lock'), path.join(workspace, 'Cargo.lock'));
  copyFileSync(path.join(source, 'build.rs'), path.join(workspace, 'build.rs'));
  for (const dir of ['src', 'capabilities', 'icons', 'bin']) {
    const from = path.join(source, dir);
    const to = path.join(workspace, dir);
    rmSync(to, { recursive: true, force: true });
    if (existsSync(from)) symlinkSync(from, to, process.platform === 'win32' ? 'junction' : 'dir');
  }

  const config = JSON.parse(readFileSync(path.join(source, 'tauri.conf.json'), 'utf8'));
  // A normal GUI/server build consumes the repository's dist/. Never replace
  // it while building this independent Cargo graph.
  config.build.frontendDist = path.join(source, 'target/ohos-frontend');
  writeFileSync(path.join(workspace, 'tauri.conf.json'), JSON.stringify(config, null, 2) + '\n');
  return workspace;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)));
  const workspace = prepareWorkspace(root, path.join(root, 'src-tauri/target/ohos-workspace'));
  process.stdout.write(workspace + '\n');
}
