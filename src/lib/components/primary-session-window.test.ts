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
  it("reconciles before releasing resource panes and awaiting the startup action", async () => {
    const reconcile = deferred<void>();
    const startupAction = deferred<void>();
    const events: string[] = [];
    const initializing = initializePrimarySessionWindow({
      reconcile: () => {
        events.push("reconcile");
        return reconcile.promise;
      },
      allowResourcePanes: () => events.push("release"),
      runStartupAction: () => {
        events.push("startup");
        return startupAction.promise;
      },
    }).then(() => events.push("complete"));

    await Promise.resolve();
    expect(events).toEqual(["reconcile"]);

    reconcile.resolve();
    await vi.waitFor(() => expect(events).toEqual(["reconcile", "release", "startup"]));
    startupAction.resolve();
    await initializing;
    expect(events).toEqual(["reconcile", "release", "startup", "complete"]);
  });

  it("releases resources and runs startup only after a successful reconciliation retry", async () => {
    const reconcile = vi.fn()
      .mockRejectedValueOnce(new Error("close failed"))
      .mockResolvedValueOnce(1);
    const allowResourcePanes = vi.fn();
    const runStartupAction = vi.fn(async () => {});
    const dependencies = { reconcile, allowResourcePanes, runStartupAction };

    await expect(initializePrimarySessionWindow(dependencies)).rejects.toThrow("close failed");
    expect(allowResourcePanes).not.toHaveBeenCalled();
    expect(runStartupAction).not.toHaveBeenCalled();

    await initializePrimarySessionWindow(dependencies);
    expect(allowResourcePanes).toHaveBeenCalledOnce();
    expect(runStartupAction).toHaveBeenCalledOnce();
  });

  it("propagates unexpected startup action failures to the startup error handler", async () => {
    const allowResourcePanes = vi.fn();
    await expect(initializePrimarySessionWindow({
      reconcile: async () => {},
      allowResourcePanes,
      runStartupAction: async () => { throw new Error("unexpected startup failure"); },
    })).rejects.toThrow("unexpected startup failure");
    expect(allowResourcePanes).toHaveBeenCalledOnce();
  });

  it("does nothing when its owner is already cancelled", async () => {
    const controller = new AbortController();
    controller.abort();
    const reconcile = vi.fn(async () => {});
    const allowResourcePanes = vi.fn();
    const runStartupAction = vi.fn(async () => {});

    await initializePrimarySessionWindow({
      signal: controller.signal,
      reconcile, allowResourcePanes, runStartupAction,
    });

    expect(reconcile).not.toHaveBeenCalled();
    expect(allowResourcePanes).not.toHaveBeenCalled();
    expect(runStartupAction).not.toHaveBeenCalled();
  });

  it("does not release resources or run startup after cancellation during reconciliation", async () => {
    const reconcile = deferred<void>();
    const controller = new AbortController();
    const allowResourcePanes = vi.fn();
    const runStartupAction = vi.fn(async () => {});
    const initializing = initializePrimarySessionWindow({
      signal: controller.signal,
      reconcile: () => reconcile.promise,
      allowResourcePanes, runStartupAction,
    });

    controller.abort();
    reconcile.resolve();
    await initializing;

    expect(allowResourcePanes).not.toHaveBeenCalled();
    expect(runStartupAction).not.toHaveBeenCalled();
  });
});
