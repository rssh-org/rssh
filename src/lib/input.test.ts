import { afterEach, describe, expect, it, vi } from "vitest";
import { installInputTracking, isTouchContextMenu, supportsTouch } from "./input.ts";

afterEach(() => vi.unstubAllGlobals());

describe("input hardware", () => {
  it("detects a touchscreen independently of UA or window width", () => {
    vi.stubGlobal("navigator", { userAgent: "desktop", maxTouchPoints: 10 });
    vi.stubGlobal("window", { innerWidth: 1600 });
    expect(supportsTouch()).toBe(true);
    vi.stubGlobal("navigator", { userAgent: "mobile", maxTouchPoints: 0 });
    vi.stubGlobal("window", { innerWidth: 360, matchMedia: () => ({ matches: false }) });
    expect(supportsTouch()).toBe(false);
  });

  it("also accepts the browser's coarse pointer capability", () => {
    vi.stubGlobal("navigator", { maxTouchPoints: 0 });
    vi.stubGlobal("window", { matchMedia: (query: string) => ({ matches: query === "(any-pointer: coarse)" }) });
    expect(supportsTouch()).toBe(true);
  });
});

describe("context menu source", () => {
  it("uses the event's pointer type when available", () => {
    expect(isTouchContextMenu({ pointerType: "touch" } as PointerEvent)).toBe(true);
    expect(isTouchContextMenu({ pointerType: "pen" } as PointerEvent)).toBe(true);
    expect(isTouchContextMenu({ pointerType: "mouse" } as PointerEvent)).toBe(false);
  });

  it("tracks mixed input for older WebViews and clears touch after a keyboard event", () => {
    const target = new EventTarget();
    const cleanup = installInputTracking(target as Document);
    const menu = {} as MouseEvent;
    try {
      target.dispatchEvent(Object.assign(new Event("pointerdown"), { pointerType: "touch" }));
      expect(isTouchContextMenu(menu)).toBe(true);
      target.dispatchEvent(new Event("keydown"));
      expect(isTouchContextMenu(menu)).toBe(false);
      target.dispatchEvent(Object.assign(new Event("pointerdown"), { pointerType: "pen" }));
      expect(isTouchContextMenu(menu)).toBe(true);
      target.dispatchEvent(Object.assign(new Event("pointerdown"), { pointerType: "mouse" }));
      expect(isTouchContextMenu(menu)).toBe(false);
    } finally {
      cleanup();
    }
    target.dispatchEvent(Object.assign(new Event("pointerdown"), { pointerType: "touch" }));
    expect(isTouchContextMenu(menu)).toBe(false);
  });

  it("detaches both listeners before tracking input on a new target", () => {
    const oldTarget = new EventTarget();
    const cleanupOld = installInputTracking(oldTarget as Document);
    oldTarget.dispatchEvent(Object.assign(new Event("pointerdown"), { pointerType: "touch" }));
    cleanupOld();

    const target = new EventTarget();
    const cleanup = installInputTracking(target as Document);
    const menu = {} as MouseEvent;
    try {
      target.dispatchEvent(Object.assign(new Event("pointerdown"), { pointerType: "pen" }));
      oldTarget.dispatchEvent(new Event("keydown"));
      expect(isTouchContextMenu(menu)).toBe(true);
      oldTarget.dispatchEvent(Object.assign(new Event("pointerdown"), { pointerType: "mouse" }));
      expect(isTouchContextMenu(menu)).toBe(true);
    } finally {
      cleanup();
    }
  });
});
