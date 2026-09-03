import { beforeEach, describe, expect, it } from "vitest";

// Stub before the module under test loads — same trap as the plugin/ai stores.
const storage = new Map<string, string>();
(globalThis as Record<string, unknown>).localStorage = {
  getItem: (k: string) => storage.get(k) ?? null,
  setItem: (k: string, v: string) => void storage.set(k, String(v)),
  removeItem: (k: string) => void storage.delete(k),
  clear: () => storage.clear(),
};

const { createSidePanelState } = await import("./panel-state.svelte.ts");

beforeEach(() => {
  storage.clear();
});

describe("open flags", () => {
  it("opens, closes and toggles per tab", () => {
    const panel = createSidePanelState({ minWidth: 280 });
    expect(panel.isOpen("a")).toBe(false);
    panel.openPanel("a");
    expect(panel.isOpen("a")).toBe(true);
    expect(panel.isOpen("b")).toBe(false);
    panel.togglePanel("a");
    expect(panel.isOpen("a")).toBe(false);
    panel.togglePanel("a");
    expect(panel.isOpen("a")).toBe(true);
    panel.closePanel("a");
    expect(panel.isOpen("a")).toBe(false);
  });
});

describe("width preference", () => {
  it("defaults to null and round-trips values", () => {
    const panel = createSidePanelState({ minWidth: 280 });
    expect(panel.width("a")).toBeNull();
    expect(panel.hasWidth("a")).toBe(false);
    panel.setWidth("a", 480);
    expect(panel.width("a")).toBe(480);
    expect(panel.hasWidth("a")).toBe(true);
    panel.setWidth("a", null);
    expect(panel.width("a")).toBeNull();
    expect(panel.hasWidth("a")).toBe(true); // explicit null ≠ no preference
  });

  it("clearTab drops the open flag and the width preference", () => {
    const panel = createSidePanelState({ minWidth: 280 });
    panel.openPanel("a");
    panel.setWidth("a", 480);
    panel.clearTab("a");
    expect(panel.isOpen("a")).toBe(false);
    expect(panel.width("a")).toBeNull();
    expect(panel.hasWidth("a")).toBe(false);
  });

  it("closeAll drops every open flag but keeps width preferences", () => {
    const panel = createSidePanelState({ minWidth: 280 });
    panel.openPanel("a");
    panel.openPanel("b");
    panel.setWidth("a", 480);
    panel.closeAll();
    expect(panel.isOpen("a")).toBe(false);
    expect(panel.isOpen("b")).toBe(false);
    expect(panel.width("a")).toBe(480);
  });
});

describe("persistence (storageKey present)", () => {
  it("commits a tab's width as the persisted default; null clears it", () => {
    const panel = createSidePanelState({ minWidth: 280, storageKey: "p1" });
    panel.setWidth("a", 480);
    expect(panel.commitWidth("a")).toBe(true);
    expect(storage.get("p1")).toBe("480");
    panel.setWidth("a", null);
    expect(panel.commitWidth("a")).toBe(true);
    expect(storage.has("p1")).toBe(false);
  });

  it("rejects commits of invalid widths without touching storage", () => {
    const panel = createSidePanelState({ minWidth: 280, storageKey: "p2" });
    panel.setWidth("a", 100); // below minWidth
    expect(panel.commitWidth("a")).toBe(false);
    expect(storage.has("p2")).toBe(false);
    expect(panel.commitWidth("never-dragged")).toBe(false);
  });

  it("seeds new tabs with the committed default loaded from storage", () => {
    storage.set("p3", "520");
    const panel = createSidePanelState({ minWidth: 280, storageKey: "p3" });
    panel.seedWidth("fresh");
    expect(panel.width("fresh")).toBe(520);
    // Seeding never overwrites an explicit preference.
    panel.setWidth("fresh", 300);
    panel.seedWidth("fresh");
    expect(panel.width("fresh")).toBe(300);
  });

  it("ignores stored values below minWidth on load", () => {
    storage.set("p4", "100");
    const panel = createSidePanelState({ minWidth: 280, storageKey: "p4" });
    panel.seedWidth("fresh");
    expect(panel.width("fresh")).toBeNull();
  });
});

describe("no storageKey (in-memory panel)", () => {
  it("commit is rejected and persists nothing; seed stays null", () => {
    const panel = createSidePanelState({ minWidth: 280 });
    panel.setWidth("a", 480);
    expect(panel.hasWidth("a")).toBe(true);
    // Without persistence there is no committed default to create — per-tab
    // widths must never leak across tabs.
    expect(panel.commitWidth("a")).toBe(false);
    expect(storage.size).toBe(0);
    panel.seedWidth("b");
    expect(panel.width("b")).toBeNull();
  });
});
