import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, closeSync, fchmodSync, lstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { gunzipSync } from 'node:zlib';

const SECRET_LIMIT = 48 * 1024;
const ARCHIVE_LIMIT = 8 * 1024 * 1024;
const ENTRY_LIMIT = 256;
const CONFIG = 'signing.local.json';
const OWNER = 'signing/.rssh-ci-signing-owner.json';

class SigningError extends Error {}

function requireSigning(condition, message) {
  if (!condition) throw new SigningError(message);
}

function requireCI(env) {
  requireSigning(env.CI === 'true', 'restore and clean require CI=true; local signing inputs are never overwritten.');
}

function present(file) {
  try { lstatSync(file); return true; } catch (error) {
    if (error.code === 'ENOENT') return false;
    throw error;
  }
}

function relativePath(name) {
  return typeof name === 'string' && name.length > 0 && !/[\\\x00-\x1f\x7f]/.test(name)
    && !name.startsWith('/') && name.split('/').every((part) => part && part !== '.' && part !== '..');
}

function signingPath(name) {
  return relativePath(name) && (name === CONFIG || name === 'signing' || name.startsWith('signing/'));
}

function collectInputs(base) {
  const entries = new Map();
  let bytes = 0;
  function visit(name) {
    if (name === OWNER) return;
    requireSigning(signingPath(name), 'Signing inputs must use relative signing/ paths.');
    const file = path.join(base, name);
    const info = lstatSync(file);
    requireSigning(info.isDirectory() || info.isFile(), 'Signing inputs cannot contain links or special files.');
    requireSigning(!info.isSymbolicLink(), 'Signing inputs cannot contain links or special files.');
    requireSigning(entries.size < ENTRY_LIMIT, 'Signing inputs contain too many files.');
    if (info.isDirectory()) {
      entries.set(name, null);
      for (const child of readdirSync(file).sort()) visit(`${name}/${child}`);
      return;
    }
    bytes += info.size;
    requireSigning(bytes <= ARCHIVE_LIMIT, 'Signing inputs exceed the 8 MiB unpacked limit.');
    entries.set(name, readFileSync(file));
  }
  visit(CONFIG);
  visit('signing');
  return entries;
}

function validateInputs(entries) {
  for (const name of entries.keys()) {
    for (let parent = path.posix.dirname(name); parent !== '.'; parent = path.posix.dirname(parent)) {
      requireSigning(entries.get(parent) === null, 'Signing archive must include directory entries for every parent path.');
    }
  }
  let config;
  try { config = JSON.parse(entries.get(CONFIG)?.toString('utf8')); } catch {
    throw new SigningError('signing.local.json must contain valid JSON.');
  }
  const configs = config?.signingConfigs;
  requireSigning(Array.isArray(configs) && configs.length === 1 && configs[0]?.name === 'default'
    && configs[0]?.type === 'HarmonyOS', 'Signing configuration must contain exactly one default HarmonyOS entry.');
  const material = configs[0].material;
  requireSigning(material && typeof material === 'object', 'Signing configuration has no material.');
  for (const field of ['storeFile', 'profile', 'certpath']) {
    const value = material[field];
    requireSigning(relativePath(value) && value.startsWith('signing/'), 'Certificate paths must be relative signing/ paths without traversal.');
    requireSigning(Buffer.isBuffer(entries.get(value)) && entries.get(value).length > 0, 'A required signing certificate, keystore or profile is missing or empty.');
  }
  for (const field of ['storePassword', 'keyPassword']) {
    const value = material[field];
    requireSigning(typeof value === 'string' && /^[0-9a-f]{32,}$/i.test(value) && value.length % 2 === 0,
      'Use DevEco encrypted storePassword/keyPassword values together with their material directory.');
  }
  requireSigning(typeof material.keyAlias === 'string' && material.keyAlias.length > 0
    && typeof material.signAlg === 'string' && material.signAlg.length > 0, 'Signing configuration requires keyAlias and signAlg.');
  const materialDir = path.posix.join(path.posix.dirname(material.storeFile), 'material');
  requireSigning(entries.get(materialDir) === null && [...entries.keys()].some((name) => name.startsWith(`${materialDir}/`)),
    'The non-empty DevEco material directory must be beside the keystore.');
  return material;
}

function writePrivateFile(file, content) {
  const fd = openSync(file, 'wx', 0o600);
  let complete = false;
  try {
    writeFileSync(fd, content);
    fchmodSync(fd, 0o600);
    complete = true;
  } finally {
    closeSync(fd);
    if (!complete) rmSync(file, { force: true });
  }
}

function writeEntries(base, entries) {
  for (const [name, bytes] of entries) {
    if (bytes === null) {
      mkdirSync(path.join(base, name), { recursive: true, mode: 0o700 });
      chmodSync(path.join(base, name), 0o700);
      continue;
    }
    mkdirSync(path.dirname(path.join(base, name)), { recursive: true, mode: 0o700 });
    writePrivateFile(path.join(base, name), bytes);
  }
}

function fingerprint(entries) {
  const hash = createHash('sha256');
  for (const [name, bytes] of [...entries].sort(([a], [b]) => a.localeCompare(b))) {
    hash.update(`${name}\0${bytes === null ? 'directory' : createHash('sha256').update(bytes).digest('hex')}\0`);
  }
  return hash.digest('hex');
}

export function packSigning(root, output) {
  const base = path.join(root, 'ohos/app');
  requireSigning(!present(output), 'The output file already exists; choose a new path.');
  const entries = collectInputs(base);
  validateInputs(entries);
  const temp = mkdtempSync(path.join(tmpdir(), 'rssh-signing-'));
  chmodSync(temp, 0o700);
  try {
    const inputs = path.join(temp, 'inputs');
    mkdirSync(inputs, { mode: 0o700 });
    writeEntries(inputs, entries);
    const archive = path.join(temp, 'signing.tar.gz');
    try {
      execFileSync('tar', ['--format=ustar', '-czf', archive, '-C', inputs, CONFIG, 'signing'],
        { env: { ...process.env, COPYFILE_DISABLE: '1' }, stdio: 'pipe' });
    } catch { throw new SigningError('Cannot create a USTAR gzip bundle; check that tar is installed.'); }
    const encoded = readFileSync(archive).toString('base64');
    requireSigning(encoded.length <= SECRET_LIMIT, 'The base64 signing bundle exceeds GitHub Secrets\' 48 KiB limit; remove unused signing files.');
    requireSigning(fingerprint(readArchive(encoded)) === fingerprint(entries), 'The signing archive does not match its inputs.');
    writePrivateFile(output, encoded);
    return encoded.length;
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
}

function tarString(bytes) {
  const end = bytes.indexOf(0);
  try { return new TextDecoder('utf-8', { fatal: true }).decode(end === -1 ? bytes : bytes.subarray(0, end)); } catch {
    throw new SigningError('Signing archive contains an invalid UTF-8 path.');
  }
}

function tarNumber(bytes) {
  const value = tarString(bytes).trim();
  requireSigning(/^[0-7]+$/.test(value), 'Signing archive contains an invalid numeric field.');
  return Number.parseInt(value, 8);
}

// Accept only regular USTAR files/directories produced by pack. Parse before any
// writes; never hand an untrusted archive to tar's filesystem extraction logic.
function readArchive(encoded) {
  requireSigning(typeof encoded === 'string' && encoded.length > 0, 'OHOS_SIGNING_BUNDLE_BASE64 is required.');
  requireSigning(encoded.length <= SECRET_LIMIT, 'OHOS_SIGNING_BUNDLE_BASE64 exceeds GitHub Secrets\' 48 KiB limit.');
  const value = encoded.trim();
  requireSigning(/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)
    && Buffer.from(value, 'base64').toString('base64') === value, 'Signing bundle must be canonical base64.');
  let data;
  try { data = gunzipSync(Buffer.from(value, 'base64'), { maxOutputLength: ARCHIVE_LIMIT }); } catch {
    throw new SigningError('Signing bundle is invalid gzip or exceeds the 8 MiB unpacked limit.');
  }
  const entries = new Map();
  let offset = 0;
  while (offset + 512 <= data.length) {
    const header = data.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) {
      requireSigning(data.length - offset >= 1024 && data.subarray(offset).every((byte) => byte === 0), 'Signing archive has an invalid end marker.');
      return entries;
    }
    requireSigning(header.subarray(257, 263).equals(Buffer.from('ustar\0')) && header.subarray(263, 265).toString() === '00',
      'Signing archive must use USTAR format; recreate it with ci-signing.mjs pack.');
    const checksum = header.reduce((sum, byte, index) => sum + (index >= 148 && index < 156 ? 32 : byte), 0);
    requireSigning(tarNumber(header.subarray(148, 156)) === checksum, 'Signing archive header checksum is invalid.');
    const type = header[156];
    requireSigning(type === 0 || type === 48 || type === 53, 'Signing archive cannot contain links, extensions or special files.');
    const prefix = tarString(header.subarray(345, 500));
    const name = `${prefix ? `${prefix}/` : ''}${tarString(header.subarray(0, 100))}`.replace(/\/$/, '');
    requireSigning(signingPath(name) && name !== OWNER, 'Signing archive contains a path outside its allowed inputs.');
    requireSigning(!entries.has(name) && entries.size < ENTRY_LIMIT, 'Signing archive contains duplicate paths or too many entries.');
    const size = tarNumber(header.subarray(124, 136));
    requireSigning(type !== 53 || size === 0, 'Signing archive directory contains file data.');
    const end = offset + 512 + size;
    offset = end + (512 - size % 512) % 512;
    requireSigning(offset <= data.length, 'Signing archive is truncated.');
    entries.set(name, type === 53 ? null : data.subarray(end - size, end));
  }
  throw new SigningError('Signing archive has no complete end marker.');
}

export function restoreSigning(root, env = process.env, log = console.log) {
  requireCI(env);
  const base = path.join(root, 'ohos/app');
  requireSigning(!present(path.join(base, CONFIG)) && !present(path.join(base, 'signing')), 'Signing inputs already exist; refusing to overwrite them.');
  const entries = readArchive(env.OHOS_SIGNING_BUNDLE_BASE64);
  const material = validateInputs(entries);
  if (env.GITHUB_ACTIONS === 'true') {
    for (const password of new Set([material.storePassword, material.keyPassword])) log(`::add-mask::${password}`);
  }
  // Claim the directory without replacing anything. On failure, remove only
  // inputs created by this invocation; validation has already completed.
  let claimed = false;
  let configCreated = false;
  try {
    mkdirSync(path.join(base, 'signing'), { mode: 0o700 });
    claimed = true;
    const signingEntries = new Map([...entries].filter(([name]) => name !== CONFIG));
    writeEntries(base, signingEntries);
    writePrivateFile(path.join(base, CONFIG), entries.get(CONFIG));
    configCreated = true;
    writePrivateFile(path.join(base, OWNER), JSON.stringify({ version: 1, scope: runScope(env), fingerprint: fingerprint(entries) }));
  } catch (error) {
    if (configCreated) rmSync(path.join(base, CONFIG));
    if (claimed) rmSync(path.join(base, 'signing'), { recursive: true, force: true });
    throw error;
  }
}

function runScope(env) {
  return [env.GITHUB_RUN_ID ?? '', env.GITHUB_RUN_ATTEMPT ?? '', env.GITHUB_JOB ?? ''].join(':');
}

function ownedInputs(base, env) {
  if (!present(path.join(base, CONFIG)) && !present(path.join(base, 'signing'))) return false;
  const marker = path.join(base, OWNER);
  requireSigning(present(marker) && lstatSync(marker).isFile() && !lstatSync(marker).isSymbolicLink(), 'Signing inputs were not restored by this tool; refusing to clean them.');
  let owner;
  try { owner = JSON.parse(readFileSync(marker, 'utf8')); } catch {
    throw new SigningError('Signing ownership marker is invalid; refusing to clean.');
  }
  requireSigning(owner?.version === 1 && owner.scope === runScope(env), 'Signing inputs belong to another CI run; refusing to clean them.');
  requireSigning(owner.fingerprint === fingerprint(collectInputs(base)), 'Signing inputs changed since restore; refusing to clean them.');
  return true;
}

export function cleanSigning(root, env = process.env) {
  requireCI(env);
  const bases = [path.join(root, 'ohos/app'), path.join(root, 'app/gen/ohos')].filter((base) => ownedInputs(base, env));
  for (const base of bases) {
    rmSync(path.join(base, CONFIG));
    rmSync(path.join(base, 'signing'), { recursive: true });
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = fileURLToPath(new URL('../..', import.meta.url));
  const [command, output, ...extra] = process.argv.slice(2);
  try {
    requireSigning(extra.length === 0, 'Usage: ci-signing.mjs pack <output-file> | restore | clean');
    if (command === 'pack' && output) {
      console.log(`Wrote signing bundle (${packSigning(root, path.resolve(output))} base64 bytes). Keep this file private.`);
    } else if (command === 'restore' && !output) {
      restoreSigning(root);
      console.log('Restored CI signing inputs.');
    } else if (command === 'clean' && !output) {
      cleanSigning(root);
      console.log('Cleaned CI signing inputs.');
    } else throw new SigningError('Usage: ci-signing.mjs pack <output-file> | restore | clean');
  } catch (error) {
    console.error(error instanceof SigningError ? error.message : 'Signing operation failed; check paths and permissions.');
    process.exitCode = 1;
  }
}
