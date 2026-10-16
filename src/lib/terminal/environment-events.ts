import { emit } from "@tauri-apps/api/event";
import { invalidateConfiguration } from "../stores/sync.svelte.ts";

export function environmentEvent(kind: "group" | "connection", id: string): string {
  return `${kind}:changed:${id}`;
}

/** Persist first, then notify every webview. Delivery failure must not undo a save. */
export async function notifyEnvironmentChanged(kind: "group" | "connection", id: string): Promise<void> {
  invalidateConfiguration();
  try {
    await emit(environmentEvent(kind, id));
  } catch (error) {
    console.warn("[terminal] environment change notification failed:", error);
  }
}
