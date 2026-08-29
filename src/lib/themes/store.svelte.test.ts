import { beforeEach, describe, expect, it, vi } from "vitest";

// The theme store persists via the Tauri backend; stub invoke to record
// writes and serve reads for init().
const invokeMock = vi.hoisted(() => vi.fn(async () => null as unknown));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

// init() writes :root CSS variables — stub the minimum document surface.
// Node has no navigator either; platform.ts guards typeof, so a desktop
// default (isMobile=false) applies.
beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(null);
  vi.stubGlobal("document", {
    documentElement: {
      dataset: {},
      style: { setProperty: () => {} },
    },
  });
});

async function loadStore() {
  vi.resetModules();
  return import("./store.svelte.ts");
}

describe("term gpu render setting", () => {
  it("defaults to on for desktop", async () => {
    const theme = await loadStore();
    expect(theme.termGpuRender()).toBe(true);
  });

  it("persists explicit off and notifies listeners", async () => {
    const theme = await loadStore();
    const seen: boolean[] = [];
    const off = theme.registerXtermGpuListener(on => seen.push(on));
    expect(seen).toEqual([true]); // fires immediately with current value

    await theme.setTermGpuRender(false);
    expect(theme.termGpuRender()).toBe(false);
    expect(seen).toEqual([true, false]);
    expect(invokeMock).toHaveBeenCalledWith("set_setting", {
      key: "theme.term-gpu-render",
      value: "false",
    });

    await theme.setTermGpuRender(true);
    expect(seen).toEqual([true, false, true]);
    off();
  });

  it("init(): explicit persisted value overrides, absence keeps platform default", async () => {
    invokeMock.mockImplementation(async (cmd: string, args: any) => {
      if (cmd === "get_setting" && args?.key === "theme.term-gpu-render") return "false";
      return null;
    });
    let theme = await loadStore();
    await theme.init();
    expect(theme.termGpuRender()).toBe(false);

    invokeMock.mockResolvedValue(null);
    theme = await loadStore();
    await theme.init();
    expect(theme.termGpuRender()).toBe(true);
  });
});
