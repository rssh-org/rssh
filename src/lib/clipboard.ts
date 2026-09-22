import { invoke } from "@tauri-apps/api/core";
import { capabilities } from "./stores/runtime.svelte.ts";

/** Read text from the system clipboard. Errors are intentionally preserved. */
export async function readText(): Promise<string> {
  return capabilities().nativeClipboard
    ? invoke<string>("clipboard_read")
    : navigator.clipboard.readText();
}

/** Write text to the system clipboard. Errors are intentionally preserved. */
export async function writeText(text: string): Promise<void> {
  if (capabilities().nativeClipboard) {
    await invoke<void>("clipboard_write", { text });
  } else {
    await navigator.clipboard.writeText(text);
  }
}
