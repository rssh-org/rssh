import { describe, expect, it } from "vitest";
import { innerEdgeIsRight, sideStacks } from "./layout.ts";

describe("sideStacks invariants", () => {
  it("sftp is always on the side opposite ai, plugin innermost on its side", () => {
    // (aiPos, pluginPos) → expected left/right stacks, outermost first.
    // Plugin is innermost: adjacent to the terminal — last on the left stack,
    // first on the right stack.
    const cases: Array<["left" | "right", "left" | "right", string[], string[]]> = [
      ["right", "right", ["sftp"], ["plugin", "ai"]],
      ["right", "left", ["sftp", "plugin"], ["ai"]],
      ["left", "right", ["ai"], ["plugin", "sftp"]],
      ["left", "left", ["ai", "plugin"], ["sftp"]],
    ];
    for (const [aiPos, pluginPos, left, right] of cases) {
      expect(sideStacks(aiPos, pluginPos)).toEqual({left, right});
    }
  });

  it("plugin is always the stack entry adjacent to the terminal", () => {
    for (const aiPos of ["left", "right"] as const) {
      for (const pluginPos of ["left", "right"] as const) {
        const stacks = sideStacks(aiPos, pluginPos);
        const stack = stacks[pluginPos];
        const adjacentIndex = pluginPos === "left" ? stack.length - 1 : 0;
        expect(stack[adjacentIndex]).toBe("plugin");
      }
    }
  });

  it("each panel appears exactly once", () => {
    for (const aiPos of ["left", "right"] as const) {
      for (const pluginPos of ["left", "right"] as const) {
        const all = [...sideStacks(aiPos, pluginPos).left, ...sideStacks(aiPos, pluginPos).right];
        expect(all.slice().sort()).toEqual(["ai", "plugin", "sftp"]);
      }
    }
  });
});

describe("innerEdgeIsRight", () => {
  it("handle faces the terminal: right edge when docked left, left edge when docked right", () => {
    expect(innerEdgeIsRight("left")).toBe(true);
    expect(innerEdgeIsRight("right")).toBe(false);
  });
});
