import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { assertWebglAvailable } from "./webgl-support";

const { terminal, addon, disposeAddon, host, context } = vi.hoisted(() => ({
  terminal: { open: vi.fn(), loadAddon: vi.fn(), dispose: vi.fn() },
  addon: { textureAtlas: {} as object | undefined },
  disposeAddon: vi.fn(),
  host: {
    style: { cssText: "" }, inert: false, setAttribute: vi.fn(), remove: vi.fn(),
    addEventListener: vi.fn(), removeEventListener: vi.fn(), querySelector: vi.fn(),
  },
  context: { isContextLost: vi.fn() },
}));

vi.mock("@xterm/xterm", () => ({
  Terminal: vi.fn(function () { return terminal; }),
}));
vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: vi.fn(function () { return addon; }),
}));
vi.mock("./webgl-addon", () => ({ disposeWebglAddon: disposeAddon }));

describe("assertWebglAvailable", () => {
  const listeners = new Map<string, EventListener>();
  const frames = new Map<number, FrameRequestCallback>();
  let nextFrame = 0;

  function renderFrame() {
    const pending = [...frames.values()];
    frames.clear();
    pending.forEach(callback => callback(0));
  }

  function expectCleanedUp() {
    expect(listeners.size).toBe(0);
    expect(frames.size).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
    expect(terminal.dispose).toHaveBeenCalledOnce();
    expect(host.remove).toHaveBeenCalledOnce();
  }

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  beforeEach(() => {
    vi.resetAllMocks();
    vi.useFakeTimers();
    listeners.clear();
    frames.clear();
    nextFrame = 0;
    addon.textureAtlas = {};
    context.isContextLost.mockReturnValue(false);
    host.querySelector.mockReturnValue({ getContext: () => context });
    host.addEventListener.mockImplementation((type, listener) => listeners.set(type, listener));
    host.removeEventListener.mockImplementation(type => listeners.delete(type));
    vi.stubGlobal("document", {
      createElement: () => host,
      body: { appendChild: vi.fn() },
    });
    vi.stubGlobal("window", {
      setTimeout, clearTimeout,
      requestAnimationFrame: (callback: FrameRequestCallback) => {
        frames.set(++nextFrame, callback);
        return nextFrame;
      },
      cancelAnimationFrame: (id: number) => frames.delete(id),
    });
  });

  it("waits two frames for early context loss and removes the probe after success", async () => {
    const check = assertWebglAvailable();
    renderFrame();
    expect(host.remove).not.toHaveBeenCalled();
    renderFrame();
    await check;

    expect(host.addEventListener).toHaveBeenCalledWith("webglcontextlost", expect.any(Function), true);
    expect(disposeAddon).toHaveBeenCalledWith(addon);
    expectCleanedUp();
  });

  it("finishes without judging performance when animation frames are suspended", async () => {
    const check = assertWebglAvailable();
    await vi.advanceTimersByTimeAsync(100);
    await check;

    expectCleanedUp();
  });

  it("rejects native context loss during the enable check and cancels pending work", async () => {
    const check = assertWebglAvailable();
    const rejected = expect(check).rejects.toThrow("context lost");
    renderFrame();
    listeners.get("webglcontextlost")!(new Event("webglcontextlost"));
    await rejected;

    expect(disposeAddon).toHaveBeenCalledWith(addon);
    expectCleanedUp();
  });

  it("rejects initialization errors and removes the context loss listener", async () => {
    terminal.loadAddon.mockImplementation(() => { throw new Error("WebGL unavailable"); });

    await expect(assertWebglAvailable()).rejects.toThrow("WebGL unavailable");

    expect(disposeAddon).toHaveBeenCalledWith(addon);
    expectCleanedUp();
  });

  it("rejects a lost context even when the browser has not delivered its event", async () => {
    const check = assertWebglAvailable();
    renderFrame();
    context.isContextLost.mockReturnValue(true);
    renderFrame();

    await expect(check).rejects.toThrow("context lost");

    expectCleanedUp();
  });

  it("rejects an addon whose glyph atlas never initialized", async () => {
    addon.textureAtlas = undefined;

    await expect(assertWebglAvailable()).rejects.toThrow("texture atlas");

    expect(disposeAddon).toHaveBeenCalledWith(addon);
    expectCleanedUp();
  });

  it("cleans up the terminal and host even when addon disposal fails", async () => {
    disposeAddon.mockImplementation(() => { throw new Error("addon cleanup failed"); });
    const check = assertWebglAvailable();
    renderFrame();
    renderFrame();

    await expect(check).rejects.toThrow("addon cleanup failed");

    expectCleanedUp();
  });

  it("removes the host even when terminal disposal fails", async () => {
    terminal.dispose.mockImplementation(() => { throw new Error("terminal cleanup failed"); });
    const check = assertWebglAvailable();
    renderFrame();
    renderFrame();

    await expect(check).rejects.toThrow("terminal cleanup failed");

    expect(disposeAddon).toHaveBeenCalledWith(addon);
    expectCleanedUp();
  });
});
