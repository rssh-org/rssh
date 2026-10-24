import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The theme store persists via the Tauri backend; stub invoke to record
// writes and serve reads for init().
const invokeMock = vi.hoisted(() =>
  vi.fn<(command: string, args?: unknown) => Promise<unknown>>(async () => null),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
const terminalPolicyMock = vi.hoisted(() => vi.fn());
vi.mock("../stores/runtime.svelte.ts", () => ({ terminalPolicy: terminalPolicyMock }));
const assertWebglAvailableMock = vi.hoisted(() => vi.fn<() => Promise<void>>());
vi.mock("../terminal/webgl-support.ts", () => ({ assertWebglAvailable: assertWebglAvailableMock }));

// init() writes :root CSS variables — stub the minimum document surface.
beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(null);
  terminalPolicyMock.mockReset();
  terminalPolicyMock.mockReturnValue({ gpuRenderDefault: true });
  assertWebglAvailableMock.mockReset();
  assertWebglAvailableMock.mockResolvedValue(undefined);
  vi.stubGlobal("document", {
    documentElement: {
      dataset: {},
      style: { setProperty: () => {} },
    },
  });
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

async function loadStore() {
  vi.resetModules();
  return import("./store.svelte.ts");
}

describe("term font CSS variable", () => {
  /** Re-stub document with a recording setProperty; returns the writes. */
  function recordRootWrites(): [string, string][] {
    const writes: [string, string][] = [];
    vi.stubGlobal("document", {
      documentElement: {
        dataset: {},
        style: { setProperty: (k: string, v: string) => writes.push([k, v]) },
      },
    });
    return writes;
  }

  it("init() exposes the chosen font + base stack as --term-font", async () => {
    const writes = recordRootWrites();
    invokeMock.mockImplementation(async (cmd: string, args: any) => {
      if (cmd === "get_setting" && args?.key === "theme.term-font") return "Cascadia Mono";
      return null;
    });
    const theme = await loadStore();
    await theme.init();
    const font = writes.find(([k]) => k === "--term-font");
    // Chosen family quoted + prepended to the base stack — same string xterm gets.
    expect(font?.[1]).toContain('"Cascadia Mono"');
    expect(font?.[1]).toContain("JetBrainsMono Nerd Font");
  });

  it("setTermFont() refreshes --term-font", async () => {
    const writes = recordRootWrites();
    invokeMock.mockResolvedValue(null);
    const theme = await loadStore();
    await theme.setTermFont("Consolas");
    const font = writes.filter(([k]) => k === "--term-font").pop();
    expect(font?.[1]).toContain('"Consolas"');
  });
});

describe("term gpu render setting", () => {
  it("waits for enable-time validation before activating or saving GPU", async () => {
    terminalPolicyMock.mockReturnValue({ gpuRenderDefault: false });
    let finish!: () => void;
    assertWebglAvailableMock.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    const theme = await loadStore();
    const seen: boolean[] = [];
    const off = theme.registerXtermGpuListener(on => { seen.push(on); });

    const enabling = theme.setTermGpuRender(true);

    expect(assertWebglAvailableMock).toHaveBeenCalledOnce();
    expect(theme.termGpuRender()).toBe(false);
    expect(seen).toEqual([false]);
    expect(invokeMock).not.toHaveBeenCalledWith("set_setting", {
      key: "theme.term-gpu-render",
      value: "true",
    });

    finish();
    await enabling;
    expect(theme.termGpuRender()).toBe(true);
    expect(seen).toEqual([false, true]);
    expect(invokeMock).toHaveBeenCalledWith("set_setting", {
      key: "theme.term-gpu-render",
      value: "true",
    });
    off();
  });

  it("keeps GPU off when validation fails without an open terminal", async () => {
    terminalPolicyMock.mockReturnValue({ gpuRenderDefault: false });
    assertWebglAvailableMock.mockRejectedValue(new Error("WebGL2 unavailable"));
    const theme = await loadStore();

    await expect(theme.setTermGpuRender(true)).rejects.toThrow("WebGL2 unavailable");

    expect(assertWebglAvailableMock).toHaveBeenCalledOnce();
    expect(theme.termGpuRender()).toBe(false);
    expect(invokeMock).not.toHaveBeenCalledWith("set_setting", {
      key: "theme.term-gpu-render",
      value: "true",
    });
  });

  it("rejects context loss during validation before enabling live renderers", async () => {
    terminalPolicyMock.mockReturnValue({ gpuRenderDefault: false });
    assertWebglAvailableMock.mockRejectedValue(new Error("WebGL context lost during initialization"));
    const theme = await loadStore();
    const listener = vi.fn((_on: boolean) => {});
    const off = theme.registerXtermGpuListener(listener);

    await expect(theme.setTermGpuRender(true)).rejects.toThrow("WebGL context lost during initialization");

    expect(listener).not.toHaveBeenCalledWith(true);
    expect(theme.termGpuRender()).toBe(false);
    expect(invokeMock).not.toHaveBeenCalledWith("set_setting", {
      key: "theme.term-gpu-render",
      value: "true",
    });
    off();
  });

  it.each(["resolve", "reject"] as const)(
    "ignores a pending enable that later %ss after the user disables GPU",
    async outcome => {
      terminalPolicyMock.mockReturnValue({ gpuRenderDefault: false });
      let resolve!: () => void;
      let reject!: (error: Error) => void;
      assertWebglAvailableMock.mockImplementation(() => new Promise((done, fail) => {
        resolve = done;
        reject = fail;
      }));
      const theme = await loadStore();
      const listener = vi.fn((_on: boolean) => {});
      const off = theme.registerXtermGpuListener(listener);
      const enabling = theme.setTermGpuRender(true);

      await theme.setTermGpuRender(false);
      if (outcome === "resolve") resolve();
      else reject(new Error("WebGL context lost during initialization"));
      await expect(enabling).resolves.toBeUndefined();

      expect(theme.termGpuRender()).toBe(false);
      expect(listener).not.toHaveBeenCalledWith(true);
      expect(assertWebglAvailableMock).toHaveBeenCalledOnce();
      expect(invokeMock).toHaveBeenCalledExactlyOnceWith("set_setting", {
        key: "theme.term-gpu-render",
        value: "false",
      });
      off();
    },
  );

  it("does not let an older failed check roll back a newer successful enable", async () => {
    terminalPolicyMock.mockReturnValue({ gpuRenderDefault: false });
    let reject!: (error: Error) => void;
    assertWebglAvailableMock.mockImplementationOnce(() => new Promise((_resolve, fail) => { reject = fail; }));
    const theme = await loadStore();
    const seen: boolean[] = [];
    const off = theme.registerXtermGpuListener(on => { seen.push(on); });
    const earlier = theme.setTermGpuRender(true);

    await theme.setTermGpuRender(true);
    reject(new Error("older check failed"));
    await expect(earlier).resolves.toBeUndefined();

    expect(theme.termGpuRender()).toBe(true);
    expect(seen).toEqual([false, true]);
    expect(invokeMock).toHaveBeenCalledExactlyOnceWith("set_setting", {
      key: "theme.term-gpu-render",
      value: "true",
    });
    off();
  });

  it("rolls every terminal back when a live renderer fails to initialize", async () => {
    terminalPolicyMock.mockReturnValue({ gpuRenderDefault: false });
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const theme = await loadStore();
    const first = vi.fn((_on: boolean) => {});
    const broken = vi.fn((on: boolean) => {
      if (on) throw new Error("renderer initialization failed");
    });
    const last = vi.fn((_on: boolean) => {});
    const unregister = [first, broken, last].map(theme.registerXtermGpuListener);

    await expect(theme.setTermGpuRender(true)).rejects.toThrow();

    expect(first.mock.calls.map(([on]) => on)).toEqual([false, true, false]);
    expect(broken).toHaveBeenLastCalledWith(false);
    expect(last).toHaveBeenLastCalledWith(false);
    expect(theme.termGpuRender()).toBe(false);
    expect(assertWebglAvailableMock).toHaveBeenCalledOnce();
    expect(invokeMock).not.toHaveBeenCalledWith("set_setting", {
      key: "theme.term-gpu-render",
      value: "true",
    });
    unregister.forEach(off => off());
  });

  it("allows a successful retry after unavailable WebGL", async () => {
    terminalPolicyMock.mockReturnValue({ gpuRenderDefault: false });
    assertWebglAvailableMock.mockRejectedValueOnce(new Error("WebGL2 unavailable"));
    const theme = await loadStore();
    await expect(theme.setTermGpuRender(true)).rejects.toThrow();

    await theme.setTermGpuRender(true);

    expect(assertWebglAvailableMock).toHaveBeenCalledTimes(2);
    expect(theme.termGpuRender()).toBe(true);
    expect(invokeMock).toHaveBeenCalledExactlyOnceWith("set_setting", {
      key: "theme.term-gpu-render",
      value: "true",
    });
  });

  it("can disable GPU without probing unavailable WebGL", async () => {
    assertWebglAvailableMock.mockRejectedValue(new Error("WebGL2 unavailable"));
    const theme = await loadStore();

    await theme.setTermGpuRender(false);

    expect(assertWebglAvailableMock).not.toHaveBeenCalled();
    expect(theme.termGpuRender()).toBe(false);
    expect(invokeMock).toHaveBeenCalledWith("set_setting", {
      key: "theme.term-gpu-render",
      value: "false",
    });
  });

  it("keeps registration usable when a restored renderer cannot initialize", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const theme = await loadStore();
    const broken = vi.fn(() => { throw new Error("WebGL2 unavailable"); });

    const unregister = theme.registerXtermGpuListener(broken);
    unregister();
    await theme.setTermGpuRender(false);

    expect(broken).toHaveBeenCalledOnce();
    expect(theme.termGpuRender()).toBe(false);
  });

  it("continues startup and other terminals when a saved renderer fails", async () => {
    terminalPolicyMock.mockReturnValue({ gpuRenderDefault: false });
    vi.spyOn(console, "warn").mockImplementation(() => {});
    invokeMock.mockImplementation(async (cmd: string, args: any) => {
      if (cmd === "get_setting" && args?.key === "theme.term-gpu-render") return "true";
      return null;
    });
    const theme = await loadStore();
    const broken = theme.registerXtermGpuListener(on => {
      if (on) throw new Error("WebGL2 unavailable");
    });
    const seen: boolean[] = [];
    const good = theme.registerXtermGpuListener(on => { seen.push(on); });

    await expect(theme.init()).resolves.toBeUndefined();

    expect(seen).toEqual([false, true]);
    expect(theme.termGpuRender()).toBe(true);
    expect(assertWebglAvailableMock).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.some(([cmd]) => cmd === "set_setting")).toBe(false);
    broken();
    good();
  });

  it("does not restore an older saved opt-in after a failed user enable", async () => {
    terminalPolicyMock.mockReturnValue({ gpuRenderDefault: false });
    assertWebglAvailableMock.mockRejectedValue(new Error("WebGL2 unavailable"));
    let finish!: (value: unknown) => void;
    invokeMock.mockImplementation(async (cmd: string, args: any) => {
      if (cmd === "get_setting" && args?.key === "theme.term-gpu-render") {
        return new Promise(resolve => { finish = resolve; });
      }
      return null;
    });
    const theme = await loadStore();
    const initializing = theme.init();

    await expect(theme.setTermGpuRender(true)).rejects.toThrow();
    finish("true");
    await initializing;

    expect(theme.termGpuRender()).toBe(false);
  });

  it("keeps an older settings read from enabling GPU while a check is pending", async () => {
    terminalPolicyMock.mockReturnValue({ gpuRenderDefault: false });
    let finishSetting!: (value: unknown) => void;
    invokeMock.mockImplementation(async (cmd: string, args: any) => {
      if (cmd === "get_setting" && args?.key === "theme.term-gpu-render") {
        return new Promise(resolve => { finishSetting = resolve; });
      }
      return null;
    });
    let finishCheck!: () => void;
    assertWebglAvailableMock.mockImplementation(() => new Promise(resolve => { finishCheck = resolve; }));
    const theme = await loadStore();
    const initializing = theme.init();
    const listener = vi.fn((_on: boolean) => {});
    const off = theme.registerXtermGpuListener(listener);
    const enabling = theme.setTermGpuRender(true);

    finishSetting("true");
    await initializing;

    expect(theme.termGpuRender()).toBe(false);
    expect(listener).not.toHaveBeenCalledWith(true);
    finishCheck();
    await enabling;
    expect(theme.termGpuRender()).toBe(true);
    expect(listener).toHaveBeenLastCalledWith(true);
    off();
  });

  it("keeps the desktop host default even when a touchscreen is present", async () => {
    vi.stubGlobal("navigator", { maxTouchPoints: 10 });
    const theme = await loadStore();
    expect(theme.termGpuRender()).toBe(true);
  });

  it("uses the host's DOM default even without detected touch hardware", async () => {
    vi.stubGlobal("navigator", { maxTouchPoints: 0 });
    terminalPolicyMock.mockReturnValue({ gpuRenderDefault: false });
    const theme = await loadStore();
    expect(theme.termGpuRender()).toBe(false);
  });

  it("reads the loaded host policy when no explicit GPU setting exists", async () => {
    terminalPolicyMock.mockImplementation(() => {
      throw new Error("Host terminal runtime policy is not loaded");
    });
    const theme = await loadStore();
    await expect(theme.init()).resolves.toBeUndefined();
    terminalPolicyMock.mockReturnValue({ gpuRenderDefault: false });
    expect(theme.termGpuRender()).toBe(false);
    const seen: boolean[] = [];
    const off = theme.registerXtermGpuListener(value => seen.push(value));
    expect(seen).toEqual([false]);
    off();
  });

  it("keeps a saved explicit choice when host policy arrives later", async () => {
    invokeMock.mockImplementation(async (cmd: string, args: any) => {
      if (cmd === "get_setting" && args?.key === "theme.term-gpu-render") return "true";
      return null;
    });
    const theme = await loadStore();
    await theme.init();
    terminalPolicyMock.mockReturnValue({ gpuRenderDefault: false });
    expect(theme.termGpuRender()).toBe(true);
  });

  it("does not overwrite a user toggle with an older in-flight setting read", async () => {
    let finish!: (value: unknown) => void;
    invokeMock.mockImplementation(async (cmd: string, args: any) => {
      if (cmd === "get_setting" && args?.key === "theme.term-gpu-render") {
        return new Promise(resolve => { finish = resolve; });
      }
      return null;
    });
    const theme = await loadStore();
    const initializing = theme.init();
    await theme.setTermGpuRender(true);
    finish("false");
    await initializing;
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
