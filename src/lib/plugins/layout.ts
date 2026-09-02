/**
 * Side-panel composition for the content row. Replaces the old
 * `.content.ai-left` row-reverse trick, which cannot express a third panel:
 * with explicit stacks every aside is placed by data, and each aside knows
 * which edge faces the terminal (stack side) for its resize handle.
 *
 * Invariants:
 *   - SFTP always docks on the side OPPOSITE the AI panel (existing behavior).
 *   - The plugin side panel docks INNERMOST on its configured side — directly
 *     against the terminal — so its resize handle faces the main area and
 *     dragging never has to negotiate across another panel.
 */

export type PanelKind = "sftp" | "plugin" | "ai";
export type HorizontalSide = "left" | "right";

/** Container size used until a plugin reports its content size (bridge
 *  "size" notification) — and forever for plugins that never report. */
export const FALLBACK_PLUGIN_SIZE = 180;

export interface SideStacks {
  left: PanelKind[];
  right: PanelKind[];
}

/** Inner edge of an aside = the edge facing the main area. */
export function innerEdgeIsRight(side: HorizontalSide): boolean {
  return side === "left";
}

export function sideStacks(aiPos: HorizontalSide, pluginPos: HorizontalSide): SideStacks {
  const sftpSide: HorizontalSide = aiPos === "left" ? "right" : "left";
  const stacks: Record<HorizontalSide, PanelKind[]> = { left: [], right: [] };
  // Stack arrays are outermost → innermost, matching DOM order in a flex row
  // (left stack renders at the window edge first; right stack ends there).
  for (const side of ["left", "right"] as const) {
    if (aiPos === side) stacks[side].push("ai");
    if (sftpSide === side) stacks[side].push("sftp");
  }
  // Plugin is innermost (adjacent to the terminal): last on the left stack,
  // first on the right stack.
  if (pluginPos === "right") stacks.right.unshift("plugin");
  else stacks.left.push("plugin");
  return stacks;
}
