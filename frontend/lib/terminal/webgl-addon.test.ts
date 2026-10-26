import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { runInNewContext } from "node:vm";
import type { WebglAddon } from "@xterm/addon-webgl";
import ts from "typescript";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { disposeWebglAddon } from "./webgl-addon.ts";

// Exercise the installed dependency's timer implementation, without needing a
// WebGL context. Transpilation removes its type-only xterm service dependency.
const managerPath = createRequire(import.meta.url).resolve(
  "@xterm/addon-webgl/src/CursorBlinkStateManager.ts",
);
const managerCode = ts.transpileModule(readFileSync(managerPath, "utf8"), {
  compilerOptions: {
    target: ts.ScriptTarget.ES2021,
    module: ts.ModuleKind.CommonJS,
  },
}).outputText;

function timerWindow() {
  return {
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
    requestAnimationFrame: (callback: FrameRequestCallback) =>
      setTimeout(() => callback(Date.now()), 16),
    cancelAnimationFrame: clearTimeout,
  };
}

type BlinkManager = { dispose(): void };
type BlinkConstructor = new (
  render: () => void,
  service: { isFocused: boolean; window: ReturnType<typeof timerWindow> },
) => BlinkManager;

function setup() {
  const exports = {} as { CursorBlinkStateManager: BlinkConstructor };
  runInNewContext(managerCode, { exports, Date }, { filename: managerPath });
  const render = vi.fn();
  const manager = new exports.CursorBlinkStateManager(render, {
    isFocused: true,
    window: timerWindow(),
  });
  const dispose = vi.fn();
  const addon = {
    _renderer: { _cursorBlinkStateManager: manager },
    dispose,
  } as unknown as WebglAddon;
  return { addon, dispose, render };
}

describe("disposeWebglAddon", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("cancels the pending blink startup timeout", () => {
    const { addon, dispose, render } = setup();
    expect(vi.getTimerCount()).toBe(1);

    disposeWebglAddon(addon);

    expect(dispose).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(2400);
    expect(render).not.toHaveBeenCalled();
  });

  it("cancels the running blink interval and its pending animation frame", () => {
    const { addon, render } = setup();
    vi.advanceTimersByTime(1200);
    expect(render).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(2);

    disposeWebglAddon(addon);

    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(2400);
    expect(render).toHaveBeenCalledOnce();
  });

  it("does not accumulate timers across lifecycles or stop another terminal", () => {
    const live = setup();
    const closed = [];

    for (let i = 0; i < 10; i++) {
      const temporary = setup();
      vi.advanceTimersByTime(616);
      expect(temporary.render).toHaveBeenCalledOnce();

      disposeWebglAddon(temporary.addon);
      closed.push(temporary);
      expect(vi.getTimerCount()).toBe(1);
    }

    expect(live.render).toHaveBeenCalledTimes(10);
    for (const terminal of closed) {
      expect(terminal.render).toHaveBeenCalledOnce();
      expect(terminal.dispose).toHaveBeenCalledOnce();
    }
    disposeWebglAddon(live.addon);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("tolerates absent addons and optional renderer internals", () => {
    disposeWebglAddon(null);
    disposeWebglAddon(undefined);

    for (const internals of [{}, { _renderer: {} }]) {
      const dispose = vi.fn();
      disposeWebglAddon({ ...internals, dispose } as unknown as WebglAddon);
      expect(dispose).toHaveBeenCalledOnce();
    }
    expect(vi.getTimerCount()).toBe(0);
  });

  it("still disposes the addon if private blink cleanup throws", () => {
    const error = new Error("blink cleanup failed");
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const dispose = vi.fn();
    const addon = {
      _renderer: {
        _cursorBlinkStateManager: { dispose() { throw error; } },
      },
      dispose,
    } as unknown as WebglAddon;

    disposeWebglAddon(addon);

    expect(dispose).toHaveBeenCalledOnce();
    expect(warn).toHaveBeenCalledWith(expect.any(String), error);
  });
});
