/** Save text through the native host, or a Blob download in a browser. */
export interface SaveOpts {
  defaultName: string;
  filters?: { name: string; extensions: string[] }[];
}

export async function saveTextFile(content: string, opts: SaveOpts): Promise<string | null> {
  // Off-Tauri: no plugin runtime. The IDE plugin (JCEF) drops downloads, so it
  // gets a clear message; a plain browser rides a Blob download.
  if (!(window as any).__TAURI_INTERNALS__ || (window as any).__RSSH_IPC_SHIM__) {
    if (typeof (window as any).__RSSH_PICK__ === "function")
      return Promise.reject(
        `__rssh_err__|${JSON.stringify({ code: "file_save_unsupported_in_plugin", params: {} })}`,
      );
    downloadTextBlob(content, opts.defaultName);
    return opts.defaultName;
  }

  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<string | null>("save_text_file", {
    defaultName: opts.defaultName,
    contents: content,
    filters: opts.filters ?? [],
  });
}

/** Trigger a browser download of `content` saved as `filename`. */
function downloadTextBlob(content: string, filename: string): void {
  const url = URL.createObjectURL(new Blob([content]));
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.style.display = "none";
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** `YYYYMMDD-HHMMSS` stamp for default export filenames. */
export function fileStamp(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}
