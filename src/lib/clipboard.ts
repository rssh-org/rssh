import { invoke } from "@tauri-apps/api/core";

/** Read text from the system clipboard. Errors are intentionally preserved. */
export async function readText(): Promise<string> {
  return invoke<string>("clipboard_read");
}

/** Write text to the system clipboard. Errors are intentionally preserved. */
export async function writeText(text: string): Promise<void> {
  await invoke<void>("clipboard_write", { text });
}
