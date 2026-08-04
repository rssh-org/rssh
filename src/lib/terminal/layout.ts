export type SplitDirection = "horizontal" | "vertical";

export type TerminalLayout =
  | { kind: "leaf"; tabId: string }
  | {
      kind: "split";
      direction: SplitDirection;
      ratio: number;
      first: TerminalLayout;
      second: TerminalLayout;
    };

export const MIN_RATIO = 0.2;
export const MAX_RATIO = 0.8;

export const leaf = (tabId: string): TerminalLayout => ({ kind: "leaf", tabId });

export function normalizeRatio(value: number): number {
  return Number.isFinite(value)
    ? Math.min(MAX_RATIO, Math.max(MIN_RATIO, value))
    : 0.5;
}

function containsLeaf(layout: TerminalLayout, tabId: string): boolean {
  if (layout.kind === "leaf") return layout.tabId === tabId;
  return containsLeaf(layout.first, tabId) || containsLeaf(layout.second, tabId);
}

export function addSplit(
  layout: TerminalLayout,
  targetId: string,
  newId: string,
  direction: SplitDirection,
  ratio: number,
): TerminalLayout {
  if (containsLeaf(layout, newId)) return layout;

  if (layout.kind === "leaf") {
    return layout.tabId === targetId
      ? {
          kind: "split",
          direction,
          ratio: normalizeRatio(ratio),
          first: leaf(newId),
          second: layout,
        }
      : layout;
  }

  const first = addSplit(layout.first, targetId, newId, direction, ratio);
  if (first !== layout.first) return { ...layout, first };

  const second = addSplit(layout.second, targetId, newId, direction, ratio);
  if (second !== layout.second) return { ...layout, second };

  return layout;
}

export function removeLeaf(layout: TerminalLayout, tabId: string): TerminalLayout | null {
  if (layout.kind === "leaf") return layout.tabId === tabId ? null : layout;

  if (containsLeaf(layout.first, tabId)) {
    const first = removeLeaf(layout.first, tabId);
    return first === null ? layout.second : { ...layout, first };
  }

  if (containsLeaf(layout.second, tabId)) {
    const second = removeLeaf(layout.second, tabId);
    return second === null ? layout.first : { ...layout, second };
  }

  return layout;
}

export function collectLeafIds(layout: TerminalLayout): string[] {
  if (layout.kind === "leaf") return [layout.tabId];
  return [...collectLeafIds(layout.first), ...collectLeafIds(layout.second)];
}
