import { invoke } from "@tauri-apps/api/core";
import type { Group, TerminalTabType } from "../stores/app.svelte.ts";
import type { PaletteTerm } from "../themes/palettes.ts";

export function groupWaveColor(group?: Group | null): string | null {
  if (!group?.terminal_waves_enabled || !/^#[0-9a-f]{6}$/i.test(group.color)) return null;
  return group.color;
}

/** Read once before creating xterm; existing terminals keep this color. */
export async function loadTerminalWaveColor(type: TerminalTabType, meta: Record<string, string>): Promise<string | null> {
  if (type !== "ssh" && type !== "telnet" && type !== "serial") return null;
  // Telnet/serial connect from tab metadata, including when cloned. SSH resolves
  // its saved profile again on connect, so its group must use the same source.
  let groupId = meta.groupId;
  if (type === "ssh") {
    if (!meta.profileId) return null;
    const profile = await invoke<{ group_id: string | null }>("get_profile", { id: meta.profileId });
    groupId = profile.group_id ?? "";
  }
  if (!groupId) return null;
  const groups = await invoke<Group[]>("list_groups");
  return groupWaveColor(groups.find(group => group.id === groupId));
}

function transparentColor(value: string): string | null {
  const color = value.trim();
  if (/^#(?:[\da-f]{3,4}|[\da-f]{6}|[\da-f]{8})$/i.test(color)) {
    const rgb = color.length <= 5
      ? [...color.slice(1, 4)].map(channel => channel.repeat(2)).join("")
      : color.slice(1, 7);
    return `#${rgb}00`;
  }
  const rgb = color.match(/^rgba?\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})(?:\s*,\s*[\d.]+)?\s*\)$/i);
  return rgb ? `rgba(${rgb[1]}, ${rgb[2]}, ${rgb[3]}, 0)` : null;
}

/** Keep the background RGB: xterm also uses it for inverse video and OSC 11. */
export function terminalWaveTheme(theme: PaletteTerm, enabled: boolean): PaletteTerm {
  if (!enabled) return theme;
  let background = transparentColor(theme.background);
  if (!background && typeof document !== "undefined") {
    // Normalize custom CSS colors without rasterizing transparent pixels, which lose RGB.
    const context = document.createElement("canvas").getContext("2d");
    if (context) {
      context.fillStyle = theme.background;
      background = transparentColor(context.fillStyle);
    }
  }
  return { ...theme, background: background ?? theme.background };
}
