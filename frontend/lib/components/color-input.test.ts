import { describe, expect, it } from "vitest";
import { isColorInputValid, normalizeHexColor } from "./color-input.ts";

describe("color input", () => {
  it("accepts pasted HEX values and expands shorthand to the stored six-digit format", () => {
    expect(normalizeHexColor("  #4a6cF7\n")).toBe("#4A6CF7");
    expect(normalizeHexColor("ff6b6b")).toBe("#FF6B6B");
    expect(normalizeHexColor("#aB3")).toBe("#AABB33");
    expect(normalizeHexColor("09f")).toBe("#0099FF");
    expect(normalizeHexColor("#000000")).toBe("#000000");
    expect(normalizeHexColor("#fff")).toBe("#FFFFFF");
  });

  it("rejects unfinished edits, invalid digits, alpha, and arbitrary CSS", () => {
    for (const input of ["", " ", "#", "#1", "#12", "#1234", "#12345", "#1234567",
      "#12345678", "#12gg34", "##123456", "#12 3456", "red", "rgb(1, 2, 3)",
      "#123456; background: url(example)"]) {
      expect(normalizeHexColor(input), input).toBeNull();
    }
  });

  it("preserves untouched imported highlight colors when editing other rule fields", () => {
    for (const saved of ["rgb(255, 0, 0)", "rgba(255, 0, 0, 0.5)", "#FF000080"]) {
      expect(isColorInputValid(saved, saved)).toBe(true);
      expect(isColorInputValid("#", saved)).toBe(false);
      expect(isColorInputValid("rgb(", saved)).toBe(false);
      expect(isColorInputValid("#09f", saved)).toBe(true);
    }
  });

  it("does not allow new non-HEX values or an empty saved color", () => {
    expect(isColorInputValid("rgb(255, 0, 0)")).toBe(false);
    expect(isColorInputValid("", "")).toBe(false);
    expect(isColorInputValid("#ff6b6b")).toBe(true);
  });
});
