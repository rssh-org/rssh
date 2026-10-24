import { describe, expect, it, vi } from "vitest";

import { initializePrimarySessionWindow } from "./primary-session-window.ts";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("initializePrimarySessionWindow", () => {
  it("does not duplicate a restored local terminal", async () => {
    const loadAutoOpenLocal = vi.fn(async () => true);
    const openLocal = vi.fn();
    await initializePrimarySessionWindow({
      reconcile: async () => {}, allowResourcePanes: () => {},
      restoreSessions: async () => true, loadAutoOpenLocal, openLocal,
    });
    expect(loadAutoOpenLocal).not.toHaveBeenCalled();
    expect(openLocal).not.toHaveBeenCalled();
  });

  it("propagates unexpected restore failures to the startup error handler", async () => {
    const loadAutoOpenLocal = vi.fn(async () => true);
    const openLocal = vi.fn();
    await expect(initializePrimarySessionWindow({
      reconcile: async () => {}, allowResourcePanes: () => {},
      restoreSessions: async () => { throw new Error("unexpected restore failure"); },
      loadAutoOpenLocal, openLocal,
    })).rejects.toThrow("unexpected restore failure");
    expect(loadAutoOpenLocal).not.toHaveBeenCalled();
    expect(openLocal).not.toHaveBeenCalled();
  });

  it("releases resource panes before restoring sessions and applying auto-open", async () => {
    const reconcile = deferred<void>();
    const events: string[] = [];
    const initializing = initializePrimarySessionWindow({
      reconcile: () => {
        events.push("reconcile");
        return reconcile.promise;
      },
      allowResourcePanes: () => events.push("release"),
      restoreSessions: async () => { events.push("restore"); return false; },
      loadAutoOpenLocal: async () => {
        events.push("load auto-open");
        return true;
      },
      openLocal: () => events.push("open local"),
    });

    await Promise.resolve();
    expect(events).toEqual(["reconcile"]);

    reconcile.resolve();
    await initializing;
    expect(events).toEqual([
      "reconcile",
      "release",
      "restore",
      "load auto-open",
      "open local",
    ]);
  });

  it("keeps resource panes closed when reconciliation fails so the owner can retry", async () => {
    const events: string[] = [];

    await expect(initializePrimarySessionWindow({
      reconcile: async () => {
        events.push("reconcile");
        throw new Error("backend unavailable");
      },
      allowResourcePanes: () => events.push("release"),
      restoreSessions: async () => { events.push("restore"); return false; },
      loadAutoOpenLocal: async () => {
        events.push("load auto-open");
        return false;
      },
      openLocal: () => events.push("open local"),
    })).rejects.toThrow("backend unavailable");

    expect(events).toEqual(["reconcile"]);
  });

  it("restores sessions without reading or applying auto-open when local PTY is unavailable", async () => {
    const events: string[] = [];

    await initializePrimarySessionWindow({
      canOpenLocal: false,
      reconcile: async () => events.push("reconcile"),
      allowResourcePanes: () => events.push("release"),
      restoreSessions: async () => { events.push("restore"); return false; },
      loadAutoOpenLocal: async () => {
        events.push("load auto-open");
        return true;
      },
      openLocal: () => events.push("open local"),
    });

    expect(events).toEqual(["reconcile", "release", "restore"]);
  });

  it("restores sessions and opens local only after a successful reconciliation retry", async () => {
    const reconcile = vi.fn()
      .mockRejectedValueOnce(new Error("close failed"))
      .mockResolvedValueOnce(1);
    const allowResourcePanes = vi.fn();
    const restoreSessions = vi.fn(async () => false);
    const openLocal = vi.fn();
    const dependencies = { reconcile, allowResourcePanes, restoreSessions, openLocal, loadAutoOpenLocal: async () => true };
    await expect(initializePrimarySessionWindow(dependencies)).rejects.toThrow("close failed");
    expect(allowResourcePanes).not.toHaveBeenCalled();
    expect(restoreSessions).not.toHaveBeenCalled();
    expect(openLocal).not.toHaveBeenCalled();
    await initializePrimarySessionWindow(dependencies);
    expect(allowResourcePanes).toHaveBeenCalledTimes(1);
    expect(restoreSessions).toHaveBeenCalledTimes(1);
    expect(openLocal).toHaveBeenCalledTimes(1);
  });

  it("does nothing after its owner is cancelled", async () => {
    const reconcile = deferred<void>();
    const controller = new AbortController();
    const events: string[] = [];
    const initializing = initializePrimarySessionWindow({
      signal: controller.signal,
      reconcile: () => reconcile.promise,
      allowResourcePanes: () => events.push("release"),
      restoreSessions: async () => { events.push("restore"); return false; },
      loadAutoOpenLocal: async () => true,
      openLocal: () => events.push("open local"),
    });

    controller.abort();
    reconcile.resolve();
    await initializing;

    expect(events).toEqual([]);
  });

  it("does not auto-open after cancellation during the settings read", async () => {
    const autoOpen = deferred<boolean>();
    const controller = new AbortController();
    const events: string[] = [];
    const initializing = initializePrimarySessionWindow({
      signal: controller.signal,
      reconcile: async () => {},
      allowResourcePanes: () => events.push("release"),
      restoreSessions: async () => false,
      loadAutoOpenLocal: () => autoOpen.promise,
      openLocal: () => events.push("open local"),
    });

    await vi.waitFor(() => expect(events).toEqual(["release"]));
    controller.abort();
    autoOpen.resolve(true);
    await initializing;

    expect(events).toEqual(["release"]);
  });
});
