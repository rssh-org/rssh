import type { Group } from "../stores/app.svelte.ts";

/** The persisted background field enables the accent; its hue follows the group. */
export function groupWaveColor(group?: Group | null): string | null {
  if (!group?.terminal_background || !/^#[0-9a-f]{6}$/i.test(group.color)) return null;
  return group.color;
}

export function environmentWaveLayout(paneHeight: number): { visible: boolean; showName: boolean } {
  return { visible: paneHeight >= 240, showName: paneHeight >= 320 };
}
