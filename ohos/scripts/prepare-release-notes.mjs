import { writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { releaseVersion } from './prepare-release.mjs';
import { MAX_RELEASE_NOTES_LENGTH, normalizeReleaseNotes } from './release-notes.mjs';

class ReleaseNotesError extends Error {}

export async function githubReleaseNotes({ version, repository, token }, { fetchImpl = fetch } = {}) {
  const tag = `v${releaseVersion(version, 1).version}`;
  if (typeof repository !== 'string' || !/^[A-Za-z0-9][A-Za-z0-9-]*\/[A-Za-z0-9_.-]+$/.test(repository)
      || ['.', '..'].includes(repository.split('/')[1])) {
    throw new ReleaseNotesError('GITHUB_REPOSITORY must be an owner/repository name.');
  }
  if (typeof token !== 'string' || !token.trim()) {
    throw new ReleaseNotesError('GITHUB_TOKEN is required to read the matching GitHub Release.');
  }
  const guidance = `Add release notes to GitHub Release ${tag} in ${repository}, then rerun HarmonyOS Upload.`;
  // The tag endpoint only promises published releases. Listing releases also
  // includes drafts visible to the authenticated account; never use "latest".
  // https://docs.github.com/en/rest/releases/releases#list-releases
  for (let page = 1; page <= 100; page++) {
    let response;
    try {
      response = await fetchImpl(`https://api.github.com/repos/${repository}/releases?per_page=100&page=${page}`, {
        method: 'GET',
        headers: {
          Accept: 'application/vnd.github+json', Authorization: `Bearer ${token}`,
          'X-GitHub-Api-Version': '2026-03-10',
        },
        redirect: 'error', signal: AbortSignal.timeout(30_000),
      });
    } catch {
      throw new ReleaseNotesError('GitHub release lookup failed (network, redirect, or timeout).');
    }
    if (response.status !== 200) {
      throw new ReleaseNotesError(`GitHub release lookup returned HTTP ${response.status}. Check repository access and GITHUB_TOKEN contents permission.`);
    }
    let releases;
    try {
      releases = await response.json();
    } catch {
      throw new ReleaseNotesError('GitHub release lookup returned invalid JSON.');
    }
    if (!Array.isArray(releases) || releases.length > 100
        || releases.some((release) => !release || typeof release.tag_name !== 'string')) {
      throw new ReleaseNotesError('GitHub release lookup returned an invalid release list.');
    }
    const matches = releases.filter((release) => release.tag_name === tag);
    if (matches.length > 1) throw new ReleaseNotesError(`Multiple GitHub Releases match ${tag}; resolve the duplicate releases before retrying.`);
    if (matches.length === 1) {
      const release = matches[0];
      if (release.prerelease !== false) throw new ReleaseNotesError(`GitHub Release ${tag} must be a stable release.`);
      if (typeof release.body !== 'string' || !release.body.trim()) {
        throw new ReleaseNotesError(`GitHub Release ${tag} has no release notes. ${guidance}`);
      }
      const originalLength = release.body.trim().length;
      const notes = normalizeReleaseNotes(release.body);
      return { tag, notes, originalLength, truncated: originalLength > MAX_RELEASE_NOTES_LENGTH };
    }
    if (releases.length < 100) throw new ReleaseNotesError(`No accessible GitHub Release exists for ${tag}. ${guidance}`);
  }
  throw new ReleaseNotesError(`GitHub release lookup exceeded 100 pages without finding ${tag}.`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv.length !== 3) throw new ReleaseNotesError('Usage: node ohos/scripts/prepare-release-notes.mjs <stable-version-or-tag>');
    const file = process.env.AGC_RELEASE_NOTES_FILE;
    const temp = process.env.RUNNER_TEMP;
    if (!file || !temp || !path.isAbsolute(file) || !path.isAbsolute(temp)) {
      throw new ReleaseNotesError('Set AGC_RELEASE_NOTES_FILE to a file inside RUNNER_TEMP.');
    }
    const relative = path.relative(temp, file);
    if (!relative || relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
      throw new ReleaseNotesError('AGC_RELEASE_NOTES_FILE must be inside RUNNER_TEMP.');
    }
    const result = await githubReleaseNotes({
      version: process.argv[2], repository: process.env.GITHUB_REPOSITORY, token: process.env.GITHUB_TOKEN,
    });
    try {
      await writeFile(file, result.notes, { encoding: 'utf8', mode: 0o600 });
    } catch {
      throw new ReleaseNotesError('Could not write the prepared GitHub release notes file.');
    }
    if (result.truncated) {
      console.log(`GitHub Release ${result.tag} notes exceeded ${MAX_RELEASE_NOTES_LENGTH} characters; using the first ${result.notes.length} for AGC.`);
    } else {
      console.log(`Prepared GitHub Release ${result.tag} notes for AGC (${result.notes.length} characters).`);
    }
  } catch (error) {
    console.error(error instanceof ReleaseNotesError ? error.message : 'Could not prepare GitHub release notes; check the stable release version and configuration.');
    process.exitCode = 1;
  }
}
