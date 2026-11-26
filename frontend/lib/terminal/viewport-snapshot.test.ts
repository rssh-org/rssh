import { describe, it, expect } from "vitest";
import { readViewportSnapshot, readViewportText, type ViewportSource } from "./viewport-snapshot.ts";

/**
 * Build an xterm-shaped fake from one string per viewport row, so tests assert
 * the buffer -> snapshot mapping without a real Terminal. getLine takes an
 * ABSOLUTE row index (xterm semantics): viewport row r lives at viewportY + r.
 */
function fakeSource(
  lines: string[],
  opts: { cols?: number; rows?: number; cursorX?: number; cursorY?: number; viewportY?: number; baseY?: number } = {},
): ViewportSource {
  const cols = opts.cols ?? Math.max(1, ...lines.map((l) => l.length));
  const rows = opts.rows ?? lines.length;
  const viewportY = opts.viewportY ?? 0;
  // Default baseY == viewportY models an unscrolled terminal.
  const baseY = opts.baseY ?? viewportY;
  return {
    cols,
    rows,
    buffer: {
      active: {
        viewportY,
        baseY,
        cursorX: opts.cursorX ?? 0,
        cursorY: opts.cursorY ?? 0,
        getLine(y: number) {
          const line = lines[y - viewportY];
          if (line === undefined) return undefined;
          return {
            getCell(x: number) {
              const ch = x < line.length ? line[x] : "";
              return { getChars: () => ch, getWidth: () => 1 };
            },
          };
        },
      },
    },
  };
}

describe("readViewportSnapshot", () => {
  it("marks blank cells (empty or space) as 0", () => {
    const snap = readViewportSnapshot(fakeSource(["   ", "   "], { cols: 3 }));
    expect(snap.cols).toBe(3);
    expect(snap.rows).toBe(2);
    expect([...snap.filled]).toEqual([0, 0, 0, 0, 0, 0]);
  });

  it("marks non-blank glyphs as 1 at the right grid position", () => {
    // row0 "a b" -> ink,blank,ink ; row1 "  c" -> blank,blank,ink
    const snap = readViewportSnapshot(fakeSource(["a b", "  c"], { cols: 3 }));
    expect([...snap.filled]).toEqual([
      1, 0, 1,
      0, 0, 1,
    ]);
  });

  it("reports cursor position inside the viewport", () => {
    const snap = readViewportSnapshot(fakeSource(["xy", "zw"], { cols: 2, cursorX: 1, cursorY: 1 }));
    expect(snap.cursor).toEqual({ x: 1, y: 1 });
  });

  it("returns null cursor when out of range", () => {
    const snap = readViewportSnapshot(fakeSource(["xy"], { cols: 2, cursorX: 0, cursorY: 5 }));
    expect(snap.cursor).toBeNull();
  });

  it("maps the cursor by absolute row (baseY+cursorY) when scrolled", () => {
    // viewport shows absolute rows 8..10; cursor absolute row = baseY+cursorY = 10 -> viewport row 2
    const snap = readViewportSnapshot(
      fakeSource(["r0", "r1", "r2"], { cols: 2, rows: 3, viewportY: 8, baseY: 10, cursorX: 1, cursorY: 0 }),
    );
    expect(snap.cursor).toEqual({ x: 1, y: 2 });
  });

  it("returns null cursor when scrolled out of the viewport", () => {
    // cursor absolute row = 10, but viewport shows rows 0..2
    const snap = readViewportSnapshot(
      fakeSource(["r0", "r1", "r2"], { cols: 2, rows: 3, viewportY: 0, baseY: 10, cursorX: 0, cursorY: 0 }),
    );
    expect(snap.cursor).toBeNull();
  });

  it("reads the visible viewport when scrolled (getLine uses absolute rows)", () => {
    // viewportY=10: viewport rows 0,1 map to absolute lines 10,11
    const snap = readViewportSnapshot(fakeSource(["AA", "BB"], { cols: 2, viewportY: 10 }));
    expect([...snap.filled]).toEqual([1, 1, 1, 1]);
  });

  it("treats a missing line as all-blank", () => {
    // rows=2 but only one line present -> second row stays 0
    const snap = readViewportSnapshot(fakeSource(["ab"], { cols: 2, rows: 2 }));
    expect([...snap.filled]).toEqual([1, 1, 0, 0]);
  });
});

describe("readViewportText", () => {
  it("joins each row's glyphs into a line, right-trimmed", () => {
    expect(readViewportText(fakeSource(["abc", "hi "], { cols: 3 }))).toEqual(["abc", "hi"]);
  });

  it("keeps interior blanks as spaces for alignment", () => {
    expect(readViewportText(fakeSource(["a c"], { cols: 3 }))).toEqual(["a c"]);
  });

  it("reads the visible viewport when scrolled", () => {
    expect(readViewportText(fakeSource(["XY"], { cols: 2, viewportY: 7 }))).toEqual(["XY"]);
  });

  it("emits an empty string for a missing line", () => {
    expect(readViewportText(fakeSource(["ab"], { cols: 2, rows: 2 }))).toEqual(["ab", ""]);
  });

  it("reads the newest rows while the main terminal is scrolled into history", () => {
    const src = fakeSource(["old 0", "old 1", "old 2", "old 3", "live 0", "live 1", "live 2", "live 3", "$", ""], {
      rows: 6, baseY: 4, cursorY: 4,
    });
    expect(readViewportText(src, 5)).toEqual(["live 0", "live 1", "live 2", "live 3", "$"]);
    expect(readViewportText(src)).toEqual(["old 0", "old 1", "old 2", "old 3", "live 0", "live 1"]);
  });

  it("omits unused rows below the cursor in a newly opened terminal", () => {
    const src = fakeSource(["welcome", "$"], { rows: 24, cursorY: 1 });
    expect(readViewportText(src, 5)).toEqual(["welcome", "$"]);
  });

  it("keeps a blank cursor row and interior blank rows in the recent tail", () => {
    const src = fakeSource(["old", "one", "", "three", "four", "", ""], { cursorY: 5 });
    expect(readViewportText(src, 5)).toEqual(["one", "", "three", "four", ""]);
  });

  it("keeps output below the cursor when a full-screen program moves the cursor up", () => {
    const src = fakeSource(["header", "one", "two", "three", "four", "status", ""], { cursorY: 0 });
    expect(readViewportText(src, 3)).toEqual(["three", "four", "status"]);
  });

  it("reads at most the requested number of recent rows", () => {
    const src = fakeSource(["0", "1", "2", "3", "4", "5", "6"], { cursorY: 6 });
    expect(readViewportText(src, 5)).toEqual(["2", "3", "4", "5", "6"]);
    expect(readViewportText(src, 0)).toEqual([]);
    expect(readViewportText(fakeSource([], { rows: 0 }), 5)).toEqual([]);
  });

  it("collapses a wide glyph's trailing cell (width 0)", () => {
    // one wide char occupies cells 0 (width 2) + 1 (width 0)
    const src: ViewportSource = {
      cols: 2,
      rows: 1,
      buffer: {
        active: {
          viewportY: 0,
          baseY: 0,
          cursorX: 0,
          cursorY: 0,
          getLine: () => ({
            getCell: (x: number) =>
              x === 0
                ? { getChars: () => "中", getWidth: () => 2 }
                : { getChars: () => "", getWidth: () => 0 },
          }),
        },
      },
    };
    expect(readViewportText(src)).toEqual(["中"]);
    expect(readViewportText(src, 5)).toEqual(["中"]);
  });
});

describe("readViewportSnapshot wide glyphs", () => {
  it("skips a trailing cell with width 0 in the filled grid", () => {
    // The trailing half of a wide glyph reports width 0; it must not count as
    // ink even if some xterm builds echo the character into it.
    const src: ViewportSource = {
      cols: 2,
      rows: 1,
      buffer: {
        active: {
          viewportY: 0,
          baseY: 0,
          cursorX: 0,
          cursorY: 0,
          getLine: () => ({
            getCell: (x: number) =>
              x === 0
                ? { getChars: () => "中", getWidth: () => 2 }
                : { getChars: () => "中", getWidth: () => 0 },
          }),
        },
      },
    };
    expect([...readViewportSnapshot(src).filled]).toEqual([1, 0]);
  });
});
