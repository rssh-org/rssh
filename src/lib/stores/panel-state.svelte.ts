/**
 * Shared state skeleton for the three side panels (AI / SFTP / plugins).
 *
 * A side panel is the SAME data structure everywhere: per-tab open flags,
 * per-tab width preferences (null = responsive default), and an optional
 * committed default width persisted to localStorage. The panels differ only
 * in policy (whether they persist, when they seed new tabs, what gates
 * opening) — those stay in each store; this factory owns the structure so
 * the reactive reads are written (and audited) exactly once.
 */

export interface SidePanelConfig {
  /** Minimum sane width; values below it never persist. */
  minWidth: number;
  /** localStorage key for the committed default width. Absent = in-memory
   *  only (commit/seed become no-ops on the persistence side). */
  storageKey?: string;
}

export interface SidePanelState {
  isOpen(tabId: string): boolean;
  openPanel(tabId: string): void;
  closePanel(tabId: string): void;
  togglePanel(tabId: string): void;
  /** Tracked read: must be a plain property get so $derived consumers
   *  subscribe even when the key is missing (see the width-drag freeze bug). */
  width(tabId: string): number | null;
  setWidth(tabId: string, width: number | null): void;
  hasWidth(tabId: string): boolean;
  /** Give a tab the committed default width if it has no preference yet. */
  seedWidth(tabId: string): void;
  /** Make a tab's width the committed default (and persist it). Returns
   *  false when there is nothing valid to commit. */
  commitWidth(tabId: string): boolean;
  /** Tab closed: drop its open flag and width preference. */
  clearTab(tabId: string): void;
}

function readStoredWidth(key: string, minWidth: number): number | null {
  try {
    const raw = localStorage.getItem(key);
    if (raw === null) return null;
    const width = Number.parseInt(raw, 10);
    return Number.isFinite(width) && width >= minWidth ? width : null;
  } catch {
    return null;
  }
}

export function createSidePanelState(config: SidePanelConfig): SidePanelState {
  let openByTab = $state<Record<string, true>>({});
  let widthByTab = $state<Record<string, number | null>>({});
  let committed = config.storageKey
    ? readStoredWidth(config.storageKey, config.minWidth)
    : null;

  function persist(width: number | null): void {
    if (!config.storageKey) return;
    try {
      if (width === null) localStorage.removeItem(config.storageKey);
      else localStorage.setItem(config.storageKey, String(width));
    } catch {
      // Storage unavailable (headless test host): stay in-memory.
    }
  }

  return {
    isOpen(tabId) {
      return openByTab[tabId] === true;
    },
    openPanel(tabId) {
      openByTab[tabId] = true;
    },
    closePanel(tabId) {
      delete openByTab[tabId];
    },
    togglePanel(tabId) {
      if (openByTab[tabId] === true) delete openByTab[tabId];
      else openByTab[tabId] = true;
    },
    width(tabId) {
      return widthByTab[tabId] ?? null;
    },
    setWidth(tabId, width) {
      widthByTab[tabId] = width;
    },
    hasWidth(tabId) {
      return Object.prototype.hasOwnProperty.call(widthByTab, tabId);
    },
    seedWidth(tabId) {
      // hasWidth is only ever called from event handlers (tab lifecycle),
      // never from a $derived, so the untracked guard is fine here.
      if (!Object.prototype.hasOwnProperty.call(widthByTab, tabId)) {
        widthByTab[tabId] = committed;
      }
    },
    commitWidth(tabId) {
      // Without persistence there is no committed default at all — the
      // plugin panel's per-tab widths must never leak across tabs.
      if (!config.storageKey) return false;
      if (!Object.prototype.hasOwnProperty.call(widthByTab, tabId)) return false;
      const width = widthByTab[tabId] ?? null;
      if (width !== null && (!Number.isFinite(width) || width < config.minWidth)) {
        return false;
      }
      committed = width;
      persist(width);
      return true;
    },
    clearTab(tabId) {
      delete openByTab[tabId];
      delete widthByTab[tabId];
    },
  };
}
