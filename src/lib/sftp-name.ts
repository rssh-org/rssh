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
  // Extract the encoded document/tree id before decoding its path separators.
  const documentId = ref.match(
    /^content:\/\/[^/]+\/(?:tree\/[^/]+\/)?(?:document|tree)\/([^/?#]+)(?:[?#].*)?$/i,
  )?.[1];
  let decoded = documentId ?? ref;
  if (/^(?:file|content):\/\//i.test(ref)) {
    try {
      decoded = decodeURIComponent(decoded);
    } catch {
      /* malformed %-escape — fall back to the raw string */
    }
  }
  // Only the first colon in a SAF id separates the root; later colons are names.
  if (documentId !== undefined) decoded = decoded.replace(/^[^:\\/]*:/, "");
  return decoded.split(/[\\/]/).pop() || "";
}
