/**
 * Best-effort remote filename for an SFTP upload, derived from the local
 * source reference the dialog plugin returns. On Android that's a SAF
 * `content://` URI whose last segment encodes the display name for user-visible
 * providers (e.g. `...%2FDownload%2Freport.pdf` → `report.pdf`); opaque
 * providers yield only an id.
 * Plain filesystem paths from the headless host keep literal percent signs.
 *
 * Returns the derived name, or "" when nothing usable can be recovered (the
 * caller supplies a timestamped fallback). SAF document id prefixes are
 * stripped; ordinary filenames retain their spaces and colons.
 */
export function remoteUploadName(ref: string): string {
  let decoded = ref;
  if (/^(?:file|content):\/\//i.test(ref)) {
    try {
      decoded = decodeURIComponent(ref);
    } catch {
      /* malformed %-escape — fall back to the raw string */
    }
  }
  const separators = /^content:\/\//i.test(ref) ? /[\\/:]/ : /[\\/]/;
  return decoded.split(separators).pop() || "";
}
