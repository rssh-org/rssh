// The publishing API allows 1000 characters, but the mainland release form
// limits new version features to 500. Keep one payload valid in both regions.
// https://developer.huawei.com/consumer/cn/doc/App/agc-help-release-app-intro-0000002313477961/
export const MAX_RELEASE_NOTES_LENGTH = 500;

export function normalizeReleaseNotes(value) {
  if (typeof value !== 'string' || !value.trim()) {
    throw new Error('Release notes must contain the new version features.');
  }
  const notes = value.trim();
  if (notes.length <= MAX_RELEASE_NOTES_LENGTH) return notes;
  let result = '';
  // Respect the UTF-16 limit without splitting a surrogate pair.
  for (const character of notes) {
    if (result.length + character.length > MAX_RELEASE_NOTES_LENGTH) break;
    result += character;
  }
  return result.trimEnd();
}
