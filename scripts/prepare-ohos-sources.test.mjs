import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { prepareAbilityModules, prepareSources } from './prepare-ohos-sources.mjs';

function fixture(run) {
  const root = mkdtempSync(path.join(tmpdir(), 'rssh sources '));
  try {
    const origin = path.join(root, 'origin');
    const config = path.join(root, 'src-tauri/ohos');
    mkdirSync(origin);
    mkdirSync(path.join(config, 'patches'), { recursive: true });
    const git = (...args) => execFileSync('git', ['-C', origin, ...args], { encoding: 'utf8' }).trim();
    git('init', '--quiet');
    writeFileSync(path.join(origin, 'value.txt'), 'original\n');
    git('add', '.');
    git('-c', 'user.name=Build Test', '-c', 'user.email=test@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '--quiet', '-m', 'fixture');
    const rev = git('rev-parse', 'HEAD');
    const descriptor = { repositories: { ability: { url: origin, rev, patches: ['patches/ability.patch'] } }, abilityModules: [] };
    const save = () => writeFileSync(path.join(config, 'sources.json'), JSON.stringify(descriptor));
    save();
    writeFileSync(path.join(config, 'patches/ability.patch'), 'diff --git a/value.txt b/value.txt\n--- a/value.txt\n+++ b/value.txt\n@@ -1 +1 @@\n-original\n+patched\n');
    run({ root, origin, config, descriptor, save, rev, git });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test('sources use the exact commit and replay patches offline from the Git cache', () => {
  fixture(({ root, origin, rev, git }) => {
    // A newer upstream commit must not move the pinned source.
    writeFileSync(path.join(origin, 'newer.txt'), 'newer revision');
    git('add', '.');
    git('-c', 'user.name=Build Test', '-c', 'user.email=test@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '--quiet', '-m', 'newer');
    const sources = prepareSources(root);
    assert.equal(readFileSync(path.join(sources, 'ability/value.txt'), 'utf8'), 'patched\n');
    assert.equal(existsSync(path.join(sources, 'ability/newer.txt')), false);
    assert.equal(JSON.parse(readFileSync(path.join(sources, 'ability/.rssh-source.json'))).rev, rev);
    rmSync(origin, { recursive: true });
    writeFileSync(path.join(sources, 'ability/value.txt'), 'untracked local edit');
    prepareSources(root, { offline: true });
    assert.equal(readFileSync(path.join(sources, 'ability/value.txt'), 'utf8'), 'patched\n');
  });
});

test('an unavailable pinned commit fails offline and preserves previous generated sources', () => {
  fixture(({ root, descriptor, save }) => {
    const sources = prepareSources(root);
    descriptor.repositories.ability.rev = '1'.repeat(40);
    save();
    assert.throws(() => prepareSources(root, { offline: true }), /not cached \(offline\)/);
    assert.equal(readFileSync(path.join(sources, 'ability/value.txt'), 'utf8'), 'patched\n');
  });
});

test('a patch mismatch aborts without replacing previous generated sources', () => {
  fixture(({ root, config }) => {
    const sources = prepareSources(root);
    const patch = path.join(config, 'patches/ability.patch');
    writeFileSync(patch, readFileSync(patch, 'utf8').replace('-original', '-wrong base'));
    assert.throws(() => prepareSources(root, { offline: true }), /patch failed|does not apply/);
    assert.equal(readFileSync(path.join(sources, 'ability/value.txt'), 'utf8'), 'patched\n');
  });
});

test('HAR staging retains upstream layout and excludes dependency/build outputs', () => {
  fixture(({ root, descriptor, save }) => {
    const sources = prepareSources(root);
    const module = path.join(sources, 'ability/native_ability');
    mkdirSync(path.join(module, 'oh_modules'), { recursive: true });
    writeFileSync(path.join(module, 'index.ets'), 'export class NativeAbility {}');
    writeFileSync(path.join(module, 'oh_modules/stale'), 'old dependency');
    descriptor.abilityModules = ['native_ability'];
    save();
    prepareAbilityModules(root, sources);
    const staged = path.join(root, 'src-tauri/gen/ohos/vendor/ability/native_ability');
    assert.equal(readFileSync(path.join(staged, 'index.ets'), 'utf8'), 'export class NativeAbility {}');
    assert.equal(existsSync(path.join(staged, 'oh_modules')), false);
  });
});
