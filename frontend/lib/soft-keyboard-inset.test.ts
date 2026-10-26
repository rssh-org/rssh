import { describe, it, expect } from "vitest";
import { softKeyboardInset } from "./soft-keyboard-inset.ts";

describe("softKeyboardInset", () => {
  it("lifts the pane by the gap between its bottom and the keyboard top", () => {
    // iPhone-ish layout: pane content bottom at 810 (844 screen minus the 34px
    // home-indicator padding the app root applies), keyboard leaves 500px of
    // visual viewport → pad 310 so the keybar lands flush on the keyboard.
    expect(softKeyboardInset(810, { offsetTop: 0, height: 500 })).toBe(310);
  });

  it("accounts for a panned-down visual viewport (offsetTop > 0)", () => {
    // Keyboard top in layout coords is offsetTop + height, not just height.
    expect(softKeyboardInset(810, { offsetTop: 100, height: 500 })).toBe(210);
  });

  it("returns 0 when the pane already fits the visual viewport (keyboard closed)", () => {
    expect(softKeyboardInset(810, { offsetTop: 0, height: 810 })).toBe(0);
  });

  it("clamps to 0 when the viewport is larger than the pane (desktop / adjustResize)", () => {
    // Android resizes the webview itself: visual viewport == full height, so
    // the formula goes negative — it must collapse to a no-op, never negative.
    expect(softKeyboardInset(810, { offsetTop: 0, height: 844 })).toBe(0);
  });

  it("rounds fractional viewport geometry to whole px", () => {
    // visualViewport reports fractional sizes during the keyboard animation;
    // whole-px padding avoids a style write per sub-pixel delta.
    expect(softKeyboardInset(810, { offsetTop: 0, height: 499.6 })).toBe(310);
  });
});
