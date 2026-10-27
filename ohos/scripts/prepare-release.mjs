import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export function releaseVersion(tag, override = '') {
  const match = /^v?((0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*))$/.exec(tag);
  if (!match) throw new Error('HarmonyOS releases require a stable version such as v0.3.2.');
  const [, version, major, minor, patch] = match;
  if (+minor > 999 || +patch > 999) throw new Error('HarmonyOS minor and patch versions must be below 1000.');
  if (override !== '' && !/^[1-9]\d*$/.test(String(override))) throw new Error('OHOS_VERSION_CODE must be a positive integer.');
  const versionCode = override === '' ? +major * 1000000 + +minor * 1000 + +patch : Number(override);
  if (!Number.isSafeInteger(versionCode) || versionCode < 1 || versionCode > 2147483647) {
    throw new Error('HarmonyOS versionCode must fit a positive int32.');
  }
  return { version, versionCode };
}

function replaceVersion(block, version, name) {
  const versions = [...block.matchAll(/^version = "([^"]+)"$/gm)];
  if (versions.length !== 1) throw new Error(`Expected one package version in ${name}.`);
  return block.replace(/^version = "[^"]+"$/m, `version = "${version}"`);
}

function manifestVersion(text, version, name) {
  if (!text.startsWith('[package]\n')) throw new Error(`Expected [package] at the start of ${name}.`);
  const nextTable = text.indexOf('\n[', '[package]'.length);
  const end = nextTable < 0 ? text.length : nextTable;
  return replaceVersion(text.slice(0, end), version, name) + text.slice(end);
}

function lockVersions(text, names, version) {
  const pending = new Set(names);
  const result = text.split(/(?=^\[\[package\]\]$)/m).map((block) => {
    const name = /^name = "([^"]+)"$/m.exec(block)?.[1];
    if (!names.includes(name)) return block;
    if (!pending.delete(name) || /^source = /m.test(block)) throw new Error(`Unexpected path package ${name} in lockfile.`);
    return replaceVersion(block, version, `lockfile package ${name}`);
  }).join('');
  if (pending.size) throw new Error('Missing RSSH path package in lockfile.');
  return result;
}

// This runs only in the disposable release checkout. Both committed lockfiles
// retain every dependency version/revision; only our path-package versions change.
export function prepareRelease(root, tag, override) {
  const release = releaseVersion(tag, override);
  const changes = new Map();
  const read = (name) => readFileSync(path.join(root, name), 'utf8');
  const json = (name, mutate) => {
    const value = JSON.parse(read(name));
    mutate(value);
    changes.set(name, JSON.stringify(value, null, 2) + '\n');
  };
  json('package.json', (value) => { value.version = release.version; });
  json('package-lock.json', (value) => {
    value.version = release.version;
    value.packages[''].version = release.version;
  });
  json('app/tauri.conf.json', (value) => { value.version = release.version; });
  for (const crate of ['app', 'core', 'cli', 'ohos']) {
    const name = `${crate}/Cargo.toml`;
    changes.set(name, manifestVersion(read(name), release.version, name));
  }
  changes.set('Cargo.lock', lockVersions(read('Cargo.lock'), ['rssh-app', 'rssh-core', 'rssh-cli'], release.version));
  changes.set('ohos/Cargo.lock', lockVersions(read('ohos/Cargo.lock'), ['rssh-app', 'rssh-core', 'rssh-ohos'], release.version));
  for (const [name, contents] of changes) writeFileSync(path.join(root, name), contents);
  return release;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.env.CI !== 'true') throw new Error('Release version preparation is restricted to CI checkouts.');
    if (process.argv.length !== 3) throw new Error('Usage: node ohos/scripts/prepare-release.mjs <stable-version-or-tag>');
    const root = fileURLToPath(new URL('../..', import.meta.url));
    const { version, versionCode } = prepareRelease(root, process.argv[2], process.env.OHOS_VERSION_CODE || '');
    if (process.env.GITHUB_ENV) appendFileSync(process.env.GITHUB_ENV, `OHOS_RELEASE_VERSION=${version}\nOHOS_VERSION_CODE=${versionCode}\n`);
    console.log(`HarmonyOS ${version}, versionCode ${versionCode}`);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
