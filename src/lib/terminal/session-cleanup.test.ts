import { describe, expect, it, vi } from "vitest";
import { createSessionCleanupRegistry } from "./session-cleanup.ts";

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("session cleanup registry", () => {
  it("merges concurrent closes and explicit retries of the same session", async () => {
    const registry = createSessionCleanupRegistry();
    const scope = registry.forScope("serial:port-1", "port-1");
    const closed = deferred();
    const close = vi.fn(() => closed.promise);
    scope.requestClose("session-1", close);
    scope.requestClose("session-1", close);
    const retry = registry.retry("session-1");
    const ready = scope.wait();
    expect(close).toHaveBeenCalledTimes(1);
    closed.resolve();
    await Promise.all([retry, ready]);
    expect(registry.pending()).toEqual([]);
  });

  it("keeps errors visible and only retries when requested", async () => {
    const notify = vi.fn();
    const registry = createSessionCleanupRegistry(notify);
    const scope = registry.forScope("serial:port-1", "port-1");
    const error = new Error("device did not release");
    const close = vi.fn().mockRejectedValueOnce(error).mockResolvedValue(undefined);
    scope.requestClose("session-1", close);
    await expect(scope.wait()).rejects.toBe(error);
    expect(registry.pending()).toEqual([{
      sessionId: "session-1", scope: "serial:port-1", label: "port-1", status: { kind: "failed", error },
    }]);
    await Promise.resolve();
    expect(close).toHaveBeenCalledTimes(1);
    expect(notify).toHaveBeenCalled();
    await registry.retry("session-1");
    expect(close).toHaveBeenCalledTimes(2);
    expect(registry.pending()).toEqual([]);
  });

  it("does not let one failed port block another port or a network session", async () => {
    const registry = createSessionCleanupRegistry();
    const blocked = registry.forScope("serial:port-1", "port-1");
    const close = vi.fn(async () => { throw new Error("busy"); });
    blocked.requestClose("old-session", close);
    await expect(blocked.wait()).rejects.toThrow("busy");
    await registry.forScope("serial:port-2", "port-2").wait();
    await registry.forScope("tab:ssh-1", "server-1").wait();
    expect(close).toHaveBeenCalledTimes(1);
    expect(registry.pending()).toHaveLength(1);
  });
});
