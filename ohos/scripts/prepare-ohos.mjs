import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import JSON5 from 'json5';

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

function packageVersion(manifest) {
  let inPackage = false;
  for (const line of manifest.split('\n')) {
    if (line.startsWith('[')) inPackage = line.trim() === '[package]';
    const version = inPackage && /^version\s*=\s*"([^"]+)"/.exec(line);
    if (version) return version[1];
  }
  throw new Error('Missing Cargo package version');
}

function appVersionLock(lock, version) {
  let matches = 0;
  const result = lock.split(/(?=^\[\[package\]\]\r?$)/m).map((entry) => {
    // Only the local application's identity changes. Every dependency entry,
    // including its source, checksum and dependency edges, stays byte-for-byte.
    if (!/^name = "rssh"\r?$/m.test(entry) || /^source = /m.test(entry)) return entry;
    if (!/^version = "[^"]+"\r?$/m.test(entry)) throw new Error('Missing rssh lock version');
    matches++;
    return entry.replace(/^version = "[^"]+"/m, `version = ${JSON.stringify(version)}`);
  }).join('');
  if (matches !== 1) throw new Error(`Expected one local rssh lock entry, found ${matches}`);
  return result;
}

function sourceTargets(manifest, source) {
  let hasLibrary = false;
  const blocks = manifest.split(/(?=^\[)/m).map((block) => {
    if (!/^\[lib\]|^\[\[bin\]\]/.test(block)) return block;
    const library = block.startsWith('[lib]');
    hasLibrary ||= library;
    const target = /^path = "([^"]+)"/m.exec(block);
    if (target) return block.replace(target[0], `path = ${JSON.stringify(path.resolve(source, target[1]))}`);
    if (!library) throw new Error('Every binary target must declare its source path');
    return block.replace('[lib]', `[lib]\npath = ${JSON.stringify(path.join(source, 'src/lib.rs'))}`);
  });
  if (!hasLibrary) blocks.push(`\n[lib]\npath = ${JSON.stringify(path.join(source, 'src/lib.rs'))}\n`);
  return blocks.join('');
}

export function prepareWorkspace(root, workspace, { versionCode } = {}) {
  const source = path.join(root, 'src-tauri');
  const framework = path.join(root, 'ohos/framework');
  let manifest = readFileSync(path.join(source, 'Cargo.toml'), 'utf8');
  const config = JSON.parse(readFileSync(path.join(source, 'tauri.conf.json'), 'utf8'));
  if (typeof config.version !== 'string' || packageVersion(manifest) !== config.version) {
    throw new Error('Cargo and Tauri package versions must match before building OHOS');
  }
  const lock = appVersionLock(readFileSync(path.join(framework, 'Cargo.lock'), 'utf8'), config.version);
  const appConfig = JSON5.parse(readFileSync(path.join(root, 'ohos/app/AppScope/app.json5'), 'utf8'));
  if (versionCode !== undefined && versionCode !== '') {
    if (!/^[0-9]+$/.test(String(versionCode))) throw new Error('OHOS versionCode must be a positive integer');
    appConfig.app.versionCode = Number(versionCode);
  }
  // OpenHarmony SDK toolchains/modulecheck/app.json limits this to int32.
  if (!Number.isInteger(appConfig.app.versionCode) || appConfig.app.versionCode < 1 || appConfig.app.versionCode > 2147483647) {
    throw new Error('OHOS versionCode must be an integer between 1 and 2147483647');
  }
  appConfig.app.versionName = config.version;
  manifest = pinVersion(manifest, 'dependencies', 'tauri', '=2.11.5');
  manifest = pinVersion(manifest, 'build-dependencies', 'tauri-build', '=2.6.3');
  // Absolute target paths keep Rust's external module paths relative to the
  // real source files, even though this workspace has its own dependency graph.
  manifest = sourceTargets(manifest, source);
  const sourceRoot = path.join(source, 'target/ohos-sources');
  manifest += '\n' + readFileSync(path.join(framework, 'overlay.toml'), 'utf8')
    .replaceAll('@OHOS_SOURCES@', JSON.stringify(sourceRoot).slice(1, -1));
  mkdirSync(workspace, { recursive: true });
  writeFileSync(path.join(workspace, 'Cargo.toml'), manifest);
  writeFileSync(path.join(workspace, 'Cargo.lock'), lock);
  writeFileSync(path.join(workspace, 'Cargo.lock.expected'), lock);
  // Packaging installs this only into the generated native project. The source
  // AppScope file is never modified, including on interrupted or failed builds.
  writeFileSync(path.join(workspace, 'app.json5'), JSON.stringify(appConfig, null, 2) + '\n');
  copyFileSync(path.join(source, 'build.rs'), path.join(workspace, 'build.rs'));
  for (const dir of ['src', 'icons', 'bin']) rmSync(path.join(workspace, dir), { recursive: true, force: true });
  for (const dir of ['capabilities']) {
    const from = path.join(source, dir);
    const to = path.join(workspace, dir);
    rmSync(to, { recursive: true, force: true });
    if (existsSync(from)) symlinkSync(from, to, process.platform === 'win32' ? 'junction' : 'dir');
  }

  // A normal GUI/server build consumes the repository's dist/. Never replace
  // it while building this independent Cargo graph.
  config.build.frontendDist = path.join(source, 'target/ohos-frontend');
  if (config.bundle?.icon) config.bundle.icon = config.bundle.icon.map((icon) => path.resolve(source, icon));
  writeFileSync(path.join(workspace, 'tauri.conf.json'), JSON.stringify(config, null, 2) + '\n');
  return workspace;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = path.resolve(fileURLToPath(new URL('../..', import.meta.url)));
  const workspace = prepareWorkspace(root, path.join(root, 'src-tauri/target/ohos-workspace'), {
    versionCode: process.env.OHOS_VERSION_CODE,
  });
  process.stdout.write(workspace + '\n');
}
