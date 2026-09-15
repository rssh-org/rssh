/**
 * Remote-file preview helpers: kind classification, size caps, CodeMirror
 * language mapping and a small CSV/TSV parser. Pure functions, no Svelte.
 */

export type PreviewKind = "image" | "pdf" | "markdown" | "table" | "code";

/** Media (image/pdf) cap: rendered via Blob URL. */
export const PREVIEW_MEDIA_MAX = 20 * 1024 * 1024;
/** Text-ish cap: decoded then sliced for display. */
export const PREVIEW_TEXT_MAX = 6 * 1024 * 1024;
/** CodeMirror doc cap — keeps editor init snappy on big logs. */
export const PREVIEW_CM_MAX = 4 * 1024 * 1024;
/** Table row cap (after the header row). */
export const PREVIEW_TABLE_MAX_ROWS = 50_000;
/** Table column cap (wide GFF/tsv dumps). */
export const PREVIEW_TABLE_MAX_COLS = 100;

const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"]);

/** ext -> CodeMirror legacy-mode name. Absent means plaintext. */
const CODE_MODES: Record<string, string> = {
  sh: "shell", bash: "shell", zsh: "shell",
  py: "python",
  r: "r",
  pl: "perl", pm: "perl",
  rs: "rust",
  go: "go",
  js: "javascript", mjs: "javascript", cjs: "javascript", jsx: "javascript",
  ts: "javascript", tsx: "javascript",
  json: "javascript",
  c: "clike", h: "clike", cpp: "clike", cc: "clike", hpp: "clike", java: "clike",
  css: "css", scss: "css",
  sql: "sql",
  xml: "xml", html: "xml", htm: "xml", svelte: "xml",
  yaml: "yaml", yml: "yaml",
  toml: "toml",
  ini: "properties", conf: "properties", cfg: "properties", env: "properties",
  ps1: "powershell",
};

/** Plain-text exts shown in the code/text view without syntax highlighting. */
const PLAIN_EXTS = new Set([
  "txt", "text", "log", "out", "err", "lst", "list",
  "fasta", "fa", "fq", "fastq", "gff", "gtf", "bed", "vcf", "sam",
  "ipynb", "diff", "patch", "csv.bak",
]);

/** Classify a remote file name into a preview kind, or null when unsupported. */
export function previewKind(name: string): PreviewKind | null {
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return null;
  const ext = name.slice(dot + 1).toLowerCase();
  if (IMAGE_EXTS.has(ext)) return "image";
  if (ext === "pdf") return "pdf";
  if (ext === "md" || ext === "markdown") return "markdown";
  if (ext === "csv" || ext === "tsv") return "table";
  if (ext in CODE_MODES) return "code";
  if (PLAIN_EXTS.has(ext)) return "code";
  return null;
}

/** CodeMirror legacy-mode name for a file name (plaintext fallback). */
export function cmLanguageFor(name: string): string {
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return "plaintext";
  const ext = name.slice(dot + 1).toLowerCase();
  return CODE_MODES[ext] ?? "plaintext";
}

/** RFC-4180-ish parser for CSV/TSV: quoted fields, escaped quotes, \r\n. */
export function parseDelimited(text: string, delim: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let inQuotes = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (inQuotes) {
      if (ch === '"') {
        if (text[i + 1] === '"') { field += '"'; i++; }
        else inQuotes = false;
      } else field += ch;
    } else if (ch === '"') {
      inQuotes = true;
    } else if (ch === delim) {
      row.push(field); field = "";
    } else if (ch === "\n") {
      row.push(field); field = "";
      rows.push(row); row = [];
    } else if (ch !== "\r") {
      field += ch;
    }
  }
  if (field !== "" || row.length > 0) { row.push(field); rows.push(row); }
  return rows;
}

/** Serialize a parsed grid back to delimited text (CSV/TSV). Quotes fields
 *  containing the delimiter, quotes or line breaks; always ends with a newline
 *  so parse → serialize round-trips files that end with one. */
export function serializeDelimited(rows: string[][], delim: string): string {
  const lines: string[] = [];
  for (const row of rows) {
    const fields = row.map((f) => {
      if (f.includes(delim) || f.includes('"') || f.includes("\n") || f.includes("\r")) {
        return '"' + f.replace(/"/g, '""') + '"';
      }
      return f;
    });
    lines.push(fields.join(delim));
  }
  return lines.length ? lines.join("\n") + "\n" : "";
}

/** Decode bytes: UTF-8 with BOM handling, GB18030 fallback for legacy encodings. */
export function decodeText(bytes: Uint8Array): string {
  if (bytes.length >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) {
    return new TextDecoder("utf-8").decode(bytes.subarray(3));
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    try { return new TextDecoder("gb18030").decode(bytes); }
    catch { return new TextDecoder("utf-8").decode(bytes); }
  }
}

export function mediaMime(name: string): string {
  const dot = name.lastIndexOf(".");
  const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
  if (ext === "pdf") return "application/pdf";
  if (ext === "svg") return "image/svg+xml";
  if (ext === "jpg" || ext === "jpeg") return "image/jpeg";
  if (ext === "gif") return "image/gif";
  if (ext === "webp") return "image/webp";
  if (ext === "bmp") return "image/bmp";
  return "image/png";
}
