import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { githubReleaseNotes } from './prepare-release-notes.mjs';
import { MAX_RELEASE_NOTES_LENGTH, normalizeReleaseNotes } from './release-notes.mjs';

const options = { version: '0.3.2', repository: 'rssh-org/rssh', token: 'test-github-token' };
const release = { tag_name: 'v0.3.2', body: '修复终端输入问题。\n改善连接体验。', draft: true, prerelease: false };

function fixture(replies) {
  const calls = [];
  const fetchImpl = async (url, request) => {
    calls.push({ url, request });
    assert.equal(request.method, 'GET', 'Release note lookup must not create, edit, or publish a release.');
    assert.equal(request.redirect, 'error');
    assert.ok(request.signal instanceof AbortSignal);
    assert.equal(request.headers.Authorization, `Bearer ${options.token}`);
    assert.equal(request.headers.Accept, 'application/vnd.github+json');
    const reply = replies[calls.length - 1];
    assert.ok(reply, 'Unexpected extra release lookup.');
    return new Response(typeof reply.body === 'string' ? reply.body : JSON.stringify(reply.body), { status: reply.status ?? 200 });
  };
  return { calls, fetchImpl };
}

test('release notes come from the exact stable tag and preserve the draft body', async () => {
  const f = fixture([{ body: [
    { ...release, tag_name: 'v0.3.3', body: 'wrong version' },
    { ...release, body: ` \n${release.body}\n ` },
  ] }]);
  assert.deepEqual(await githubReleaseNotes(options, f), {
    tag: 'v0.3.2', notes: release.body, originalLength: release.body.length, truncated: false,
  });
  assert.equal(f.calls.length, 1);
  assert.equal(f.calls[0].url, 'https://api.github.com/repos/rssh-org/rssh/releases?per_page=100&page=1');
});

test('release notes paginate and also accept an already published release and v-prefixed input', async () => {
  const f = fixture([
    { body: Array.from({ length: 100 }, (_, index) => ({ ...release, tag_name: `v1.0.${index}` })) },
    { body: [{ ...release, draft: false }] },
  ]);
  assert.equal((await githubReleaseNotes({ ...options, version: 'v0.3.2' }, f)).notes, release.body);
  assert.equal(f.calls.length, 2);
  assert.match(f.calls[1].url, /page=2$/);
});

test('long notes are truncated without splitting a Unicode character or replacing the actual content', async () => {
  const notes = 'a'.repeat(MAX_RELEASE_NOTES_LENGTH - 1) + '😀继续说明';
  const f = fixture([{ body: [{ ...release, body: notes }] }]);
  const result = await githubReleaseNotes(options, f);
  assert.equal(result.notes, 'a'.repeat(MAX_RELEASE_NOTES_LENGTH - 1));
  assert.equal(result.truncated, true);
  assert.equal(result.originalLength, notes.length);
  assert.equal(normalizeReleaseNotes('新'.repeat(700)), '新'.repeat(MAX_RELEASE_NOTES_LENGTH));
  assert.equal(normalizeReleaseNotes('😀'.repeat(300)), '😀'.repeat(MAX_RELEASE_NOTES_LENGTH / 2));
  assert.equal(normalizeReleaseNotes('a'.repeat(MAX_RELEASE_NOTES_LENGTH)), 'a'.repeat(MAX_RELEASE_NOTES_LENGTH));
});

test('notes are treated as text, including Markdown and shell or workflow syntax', async () => {
  const notes = '# 更新\n- $(touch /tmp/should-not-exist)\n`command`\n::error::text\nA=1';
  const f = fixture([{ body: [{ ...release, body: notes }] }]);
  assert.equal((await githubReleaseNotes(options, f)).notes, notes);
});

for (const [name, body, expected] of [
  ['missing release', [], /No accessible GitHub Release.*v0\.3\.2.*Add release notes.*rerun/],
  ['only a different tag', [{ ...release, tag_name: '0.3.2' }], /No accessible GitHub Release/],
  ['empty notes', [{ ...release, body: ' \n ' }], /has no release notes.*rerun/],
  ['null notes', [{ ...release, body: null }], /has no release notes/],
  ['non-string notes', [{ ...release, body: { secret: 'DO-NOT-LOG' } }], /has no release notes/],
  ['prerelease', [{ ...release, prerelease: true }], /must be a stable release/],
  ['duplicate tags', [release, release], /Multiple GitHub Releases/],
  ['non-list result', { message: 'DO-NOT-LOG' }, /invalid release list/],
  ['malformed release', [null], /invalid release list/],
  ['invalid JSON', 'DO-NOT-LOG', /invalid JSON/],
]) {
  test(`${name} stops without exposing the response`, async () => {
    const f = fixture([{ body }]);
    await assert.rejects(githubReleaseNotes(options, f), (error) => {
      assert.match(error.message, expected);
      assert.doesNotMatch(error.message, /DO-NOT-LOG/);
      return true;
    });
    assert.equal(f.calls.length, 1);
  });
}

for (const status of [301, 401, 403, 404, 500]) {
  test(`HTTP ${status} fails safely without retrying`, async () => {
    const f = fixture([{ status, body: 'DO-NOT-LOG' }]);
    await assert.rejects(githubReleaseNotes(options, f), new RegExp(`HTTP ${status}`));
    assert.equal(f.calls.length, 1);
  });
}

test('transport exceptions cannot expose the GitHub token', async () => {
  await assert.rejects(githubReleaseNotes(options, {
    fetchImpl: async (_url, request) => { throw new Error(request.headers.Authorization); },
  }), { message: 'GitHub release lookup failed (network, redirect, or timeout).' });
});

test('invalid configuration fails before making a request', async () => {
  for (const input of [
    { version: 'v1.0.0-beta.1' }, { version: '$(command)' },
    { repository: '../rssh' }, { repository: 'rssh-org/..' },
    { repository: 'https://evil.test/repo' }, { repository: 'rssh-org/rssh?secret' },
    { token: '' },
  ]) {
    const f = fixture([]);
    await assert.rejects(githubReleaseNotes({ ...options, ...input }, f));
    assert.equal(f.calls.length, 0);
  }
  for (const notes of [null, undefined, {}, '', ' \n ']) {
    assert.throws(() => normalizeReleaseNotes(notes), /new version features/);
  }
});

test('the CLI writes notes only to its temporary file and never logs or exports their contents', (t) => {
  const dir = mkdtempSync(path.join(tmpdir(), 'rssh release notes '));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const file = path.join(dir, 'release-notes.txt');
  const preload = path.join(dir, 'fetch.mjs');
  const notes = '# 更新\n- 修复问题\n::error::DO-NOT-LOG\nENV=DO-NOT-LOG\n$(false)';
  writeFileSync(preload, `globalThis.fetch = async () => new Response(JSON.stringify(${JSON.stringify([{ ...release, body: notes }])}), { status: 200 });`);
  const envFile = path.join(dir, 'github-env');
  const args = ['--import', preload, 'ohos/scripts/prepare-release-notes.mjs', options.version];
  const env = {
    ...process.env, GITHUB_REPOSITORY: options.repository, GITHUB_TOKEN: options.token,
    RUNNER_TEMP: dir, AGC_RELEASE_NOTES_FILE: file, GITHUB_ENV: envFile,
  };
  const result = spawnSync(process.execPath, args, { encoding: 'utf8', env });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(file, 'utf8'), notes);
  assert.doesNotMatch(result.stdout + result.stderr, /DO-NOT-LOG|test-github-token/);
  assert.equal(existsSync(envFile), false);
  const invalid = spawnSync(process.execPath, args, { encoding: 'utf8', env: { ...env, AGC_RELEASE_NOTES_FILE: path.join(dir, '..', 'outside.txt') } });
  assert.equal(invalid.status, 1);
  assert.match(invalid.stderr, /inside RUNNER_TEMP/);
});

test('the CLI reports when the GitHub notes are truncated for AGC', (t) => {
  const dir = mkdtempSync(path.join(tmpdir(), 'rssh release notes '));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const file = path.join(dir, 'release-notes.txt');
  const preload = path.join(dir, 'fetch.mjs');
  writeFileSync(preload, `globalThis.fetch = async () => new Response(JSON.stringify(${JSON.stringify([{ ...release, body: '修'.repeat(600) }])}), { status: 200 });`);
  const result = spawnSync(process.execPath, ['--import', preload, 'ohos/scripts/prepare-release-notes.mjs', 'v0.3.2'], {
    encoding: 'utf8', env: {
      ...process.env, GITHUB_REPOSITORY: options.repository, GITHUB_TOKEN: options.token,
      RUNNER_TEMP: dir, AGC_RELEASE_NOTES_FILE: file,
    },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /exceeded 500 characters; using the first 500/);
  assert.equal(readFileSync(file, 'utf8'), '修'.repeat(500));
});
