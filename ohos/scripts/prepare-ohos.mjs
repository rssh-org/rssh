import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import JSON5 from 'json5';

function packageVersion(manifest) {
  let inPackage = false;
  for (const line of manifest.split('\n')) {
    if (line.startsWith('[')) inPackage = line.trim() === '[package]';
    const version = inPackage && /^version\s*=\s*"([^"]+)"/.exec(line);
    if (version) return version[1];
  }
  throw new Error('Missing Cargo package version');
}

// Rust manifests and lockfiles are maintained inputs. Preparation only supplies
// release metadata and resource paths to the native packaging tools.
export function preparePackaging(root, { versionCode } = {}) {
  const app = path.join(root, 'app');
  const manifest = readFileSync(path.join(app, 'Cargo.toml'), 'utf8');
  const config = JSON.parse(readFileSync(path.join(app, 'tauri.conf.json'), 'utf8'));
  if (typeof config.version !== 'string' || packageVersion(manifest) !== config.version) {
    throw new Error('Cargo and Tauri package versions must match before building OHOS');
  }
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
  const overlay = {
    version: config.version,
    build: { frontendDist: path.join(root, 'target/ohos-frontend') },
    bundle: { icon: config.bundle?.icon?.map((icon) => path.resolve(app, icon)) },
  };
  const staging = path.join(root, 'target/ohos-staging');
  mkdirSync(staging, { recursive: true });
  writeFileSync(path.join(staging, 'app.json5'), JSON.stringify(appConfig, null, 2) + '\n');
  writeFileSync(path.join(staging, 'tauri-config.json'), JSON.stringify(overlay, null, 2) + '\n');
  return staging;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = path.resolve(fileURLToPath(new URL('../..', import.meta.url)));
  process.stdout.write(preparePackaging(root, { versionCode: process.env.OHOS_VERSION_CODE }) + '\n');
}
