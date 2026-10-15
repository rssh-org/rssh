import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

function git(args, options = {}) {
  return execFileSync('git', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], ...options });
}

// Sources are generated build inputs. The checked-in revisions and patches are
// authoritative; never require a developer's modified Cargo git cache.
export function prepareSources(root, { offline = false } = {}) {
  const configDir = path.join(root, 'src-tauri/ohos');
  const sources = JSON.parse(readFileSync(path.join(configDir, 'sources.json'), 'utf8'));
  const target = path.join(root, 'src-tauri/target');
  const cacheRoot = path.join(target, 'ohos-git');
  const destination = path.join(target, 'ohos-sources');
  mkdirSync(cacheRoot, { recursive: true });
  const staging = mkdtempSync(path.join(target, 'ohos-sources-next-'));
  try {
    for (const [name, source] of Object.entries(sources.repositories)) {
      if (!/^[a-z][a-z0-9-]*$/.test(name) || !/^[a-f0-9]{40}$/.test(source.rev)) {
        throw new Error(`Invalid pinned source: ${name}`);
      }
      const cache = path.join(cacheRoot, `${name}.git`);
      if (!existsSync(cache)) git(['init', '--bare', '--quiet', cache]);
      try {
        git(['--git-dir', cache, 'cat-file', '-e', `${source.rev}^{commit}`]);
      } catch {
        if (offline) throw new Error(`Pinned source ${name}@${source.rev} is not cached (offline)`);
        git(['--git-dir', cache, 'fetch', '--quiet', '--depth=1', source.url, source.rev]);
      }
      const resolved = git(['--git-dir', cache, 'rev-parse', `${source.rev}^{commit}`]).trim();
      if (resolved !== source.rev) throw new Error(`Source revision mismatch: ${name}`);
      // fetch-by-SHA only updates FETCH_HEAD. Give clone a persistent ref so
      // it does not treat this object cache as an empty repository.
      git(['--git-dir', cache, 'update-ref', 'refs/heads/pinned', source.rev]);
      const checkout = path.join(staging, name);
      git(['clone', '--quiet', '--shared', '--no-checkout', cache, checkout]);
      git(['-C', checkout, 'checkout', '--quiet', '--detach', source.rev]);
      const patches = [];
      for (const filename of source.patches ?? []) {
        const patch = path.join(configDir, filename);
        const content = readFileSync(patch);
        git(['-C', checkout, 'apply', '--check', patch]);
        git(['-C', checkout, 'apply', patch]);
        patches.push({ file: filename, sha256: createHash('sha256').update(content).digest('hex') });
      }
      writeFileSync(path.join(checkout, '.rssh-source.json'), JSON.stringify({ rev: source.rev, patches }, null, 2) + '\n');
    }
    // Replace outputs only after every revision and patch has succeeded.
    rmSync(destination, { recursive: true, force: true });
    renameSync(staging, destination);
    return destination;
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
}

export function prepareAbilityModules(root, sourceRoot) {
  const config = JSON.parse(readFileSync(path.join(root, 'src-tauri/ohos/sources.json'), 'utf8'));
  const vendor = path.join(root, 'src-tauri/gen/ohos/vendor/ability');
  // Keep upstream relative HAR dependencies intact; only copy source inputs.
  rmSync(vendor, { recursive: true, force: true });
  for (const module of config.abilityModules) {
    cpSync(path.join(sourceRoot, 'ability', module), path.join(vendor, module), {
      recursive: true,
      filter: (entry) => !['oh_modules', 'build', '.hvigor', '.cxx', 'oh-package-lock.json5'].includes(path.basename(entry)),
    });
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)));
  const sourceRoot = prepareSources(root, {
    offline: process.argv.includes('--offline') || process.env.CARGO_NET_OFFLINE === 'true',
  });
  prepareAbilityModules(root, sourceRoot);
  process.stdout.write(sourceRoot + '\n');
}
