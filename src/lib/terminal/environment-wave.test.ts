import { describe, expect, it } from "vitest";
import { groupWaveColor, environmentWaveLayout } from "./environment-wave";

describe("group terminal waves", () => {
  it("keeps small and nested split panes usable while preserving normal-size decorations", () => {
    for (const height of [0, 40, 107, 239]) {
      expect(environmentWaveLayout(height)).toEqual({visible: false, showName: false});
    }
    expect(environmentWaveLayout(240)).toEqual({visible: true, showName: false});
    expect(environmentWaveLayout(311)).toEqual({visible: true, showName: false});
    expect(environmentWaveLayout(320)).toEqual({visible: true, showName: true});
    expect(environmentWaveLayout(800)).toEqual({visible: true, showName: true});
  });
  const production = {
    id: "prod", name: "Production", color: "#E05868", sort_order: 0,
    terminal_background: "#3B1518",
  };

  it("uses the current group color rather than a stored background override", () => {
    expect(groupWaveColor(production)).toBe("#E05868");
    expect(groupWaveColor({ ...production, color: "#4A6CF7" })).toBe("#4A6CF7");
    expect(production.terminal_background).toBe("#3B1518");
  });

  it("removes the decoration when disabled, ungrouped or deleted", () => {
    expect(groupWaveColor({ ...production, terminal_background: null })).toBeNull();
    expect(groupWaveColor({ ...production, terminal_background: undefined })).toBeNull();
    expect(groupWaveColor(null)).toBeNull();
    expect(groupWaveColor()).toBeNull();
  });

  it("does not insert malformed colors into SVG styles", () => {
    for (const color of ["", "red", "#fff", "#123456; color:red"]) {
      expect(groupWaveColor({ ...production, color })).toBeNull();
    }
  });
});
