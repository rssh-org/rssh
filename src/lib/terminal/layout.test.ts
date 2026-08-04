import { describe, expect, it } from "vitest";
import {
  addSplit,
  collectLeafIds,
  leaf,
  normalizeRatio,
  removeLeaf,
} from "./layout";

const root = leaf("root");

describe("terminal layout", () => {
  it("adds horizontal and vertical siblings", () => {
    expect(addSplit(root, "root", "left", "horizontal", 0.5)).toEqual({
      kind: "split", direction: "horizontal", ratio: 0.5,
      first: { kind: "leaf", tabId: "left" },
      second: { kind: "leaf", tabId: "root" },
    });
    expect(addSplit(root, "root", "down", "vertical", 0.5)).toMatchObject({
      kind: "split", direction: "vertical",
    });
  });

  it("clamps ratios to the usable range", () => {
    expect(normalizeRatio(-1)).toBe(0.2);
    expect(normalizeRatio(1.5)).toBe(0.8);
    expect(normalizeRatio(Number.NaN)).toBe(0.5);
  });

  it("removes a leaf and promotes the remaining child", () => {
    const tree = addSplit(root, "root", "left", "horizontal", 0.5);
    expect(removeLeaf(tree, "left")).toEqual(root);
    expect(removeLeaf(tree, "missing")).toEqual(tree);
  });

  it("supports nested removal without leaving one-child splits", () => {
    const one = addSplit(root, "root", "one", "horizontal", 0.3);
    const two = addSplit(one, "one", "two", "vertical", 0.7);
    expect(collectLeafIds(removeLeaf(two, "one"))).toEqual(["two", "root"]);
  });
  it("leaves duplicate ids and missing targets unchanged", () => {
    const tree = addSplit(root, "root", "left", "horizontal", 0.5);
    expect(addSplit(tree, "root", "left", "vertical", 0.7)).toBe(tree);
    expect(addSplit(tree, "missing", "down", "vertical", 0.7)).toBe(tree);
    expect(removeLeaf(tree, "missing")).toBe(tree);
  });

  it("keeps exact ratio boundaries", () => {
    expect(normalizeRatio(0.2)).toBe(0.2);
    expect(normalizeRatio(0.8)).toBe(0.8);
  });
});
