import { describe, expect, it, vi } from "vitest";

import { createReservedSessionAttempt } from "./reserved-session-attempt.ts";
import { createSessionCleanupRegistry } from "./session-cleanup.ts";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("ReservedSessionAttempt", () => {
  it("wires events before opening the reserved session", async () => {
    const wired = deferred<() => void>();
    const openBackend = vi.fn(async (id: string) => id);
    const attempt = createReservedSessionAttempt({
      makeId: () => "session-1",
      wireEvents: () => wired.promise,
      close: vi.fn(),
    });

    const pending = attempt.open(openBackend);
    await Promise.resolve();
    expect(openBackend).not.toHaveBeenCalled();

    wired.resolve(() => {});
    await expect(pending).resolves.toEqual({ kind: "ready", sessionId: "session-1" });
    expect(openBackend).toHaveBeenCalledWith("session-1");
  });

  it("closes both the reservation and a handle that returns after cancellation", async () => {
    const opened = deferred<string>();
    const backendStarted = deferred<void>();
    const close = vi.fn();
    const attempt = createReservedSessionAttempt({
      makeId: () => "reserved-1",
      wireEvents: async () => () => {},
      close,
    });

    const pending = attempt.open(() => {
      backendStarted.resolve(undefined);
      return opened.promise;
    });
    await backendStarted.promise;

    attempt.cancel();
    expect(close).toHaveBeenCalledWith("reserved-1");

    opened.resolve("opened-1");
    await expect(pending).resolves.toEqual({ kind: "cancelled" });
    expect(close).toHaveBeenCalledWith("opened-1");
  });

  it("destroys a wiring attempt and permanently rejects later opens", async () => {
    const wired = deferred<() => void>();
    const wiringStarted = deferred<void>();
    const disposeEvents = vi.fn();
    const openBackend = vi.fn(async (id: string) => id);
    const close = vi.fn();
    const attempt = createReservedSessionAttempt({
      makeId: () => "reserved-1",
      wireEvents: () => {
        wiringStarted.resolve(undefined);
        return wired.promise;
      },
      close,
    });

    const pending = attempt.open(openBackend);
    await wiringStarted.promise;
    attempt.destroy();
    expect(close).toHaveBeenCalledWith("reserved-1");

    wired.resolve(disposeEvents);
    await expect(pending).resolves.toEqual({ kind: "cancelled" });
    expect(disposeEvents).toHaveBeenCalledOnce();
    expect(openBackend).not.toHaveBeenCalled();

    await expect(attempt.open(openBackend)).resolves.toEqual({ kind: "cancelled" });
    expect(openBackend).not.toHaveBeenCalled();
  });

  it("accepts events only for the current reservation", async () => {
    const opened = deferred<string>();
    const backendStarted = deferred<void>();
    const attempt = createReservedSessionAttempt({
      makeId: () => "reserved-1",
      wireEvents: async () => () => {},
      close: vi.fn(),
    });

    const pending = attempt.open(() => {
      backendStarted.resolve(undefined);
      return opened.promise;
    });
    await backendStarted.promise;
    expect(attempt.accepts("reserved-1")).toBe(true);
    expect(attempt.accepts("stale")).toBe(false);

    attempt.cancel();
    expect(attempt.accepts("reserved-1")).toBe(false);
    opened.resolve("opened-1");
    await pending;
  });

  it("disposes failed attempts and permits a clean retry", async () => {
    const disposeFirst = vi.fn();
    const disposeSecond = vi.fn();
    const disposers = [disposeFirst, disposeSecond];
    let nextId = 0;
    const attempt = createReservedSessionAttempt({
      makeId: () => `reserved-${++nextId}`,
      wireEvents: async () => disposers.shift()!,
      close: vi.fn(async () => {}),
    });
    const failure = new Error("open failed");

    await expect(attempt.open(async () => { throw failure; })).rejects.toBe(failure);
    expect(disposeFirst).toHaveBeenCalledOnce();
    expect(attempt.isPending()).toBe(false);
    expect(attempt.accepts("reserved-1")).toBe(false);

    await expect(attempt.open(async (id) => id)).resolves.toEqual({
      kind: "ready",
      sessionId: "reserved-2",
    });
    expect(disposeSecond).not.toHaveBeenCalled();
  });

  it("best-effort closes the reserved id when opening rejects", async () => {
    const close = vi.fn(async () => {});
    const attempt = createReservedSessionAttempt({
      makeId: () => "reserved-1",
      wireEvents: async () => () => {},
      close,
    });

    await expect(attempt.open(async () => {
      throw new Error("response lost");
    })).rejects.toThrow("response lost");

    expect(close).toHaveBeenCalledWith("reserved-1");
  });

  it("retries a failed open after its backend reservation was already closed", async () => {
    const registry = createSessionCleanupRegistry();
    const resources = new Map<string, "pending" | "ready" | "closed">();
    const timeout = new Error("TCP connection timed out after 10 seconds");
    let nextId = 0;
    let connectionAvailable = false;
    const openBackend = vi.fn(async (id: string) => {
      resources.set(id, "pending");
      if (!connectionAvailable) {
        // The backend reservation's Drop closes a failed open before its
        // rejection reaches the frontend. Closing that record is idempotent.
        resources.set(id, "closed");
        throw timeout;
      }
      resources.set(id, "ready");
      return id;
    });
    const close = vi.fn(async (id: string) => {
      if (resources.has(id)) resources.set(id, "closed");
    });
    const attempt = createReservedSessionAttempt({
      makeId: () => `reserved-${++nextId}`,
      wireEvents: async () => () => {},
      cleanup: registry.forScope("tab:ssh-1", "SSH server"),
      close,
    });

    await expect(attempt.open(openBackend)).rejects.toBe(timeout);
    expect(resources.get("reserved-1")).toBe("closed");
    expect(close).toHaveBeenCalledWith("reserved-1");
    connectionAvailable = true;
    await expect(attempt.open(openBackend)).resolves.toEqual({ kind: "ready", sessionId: "reserved-2" });
    expect(openBackend).toHaveBeenCalledTimes(2);
    expect(resources.get("reserved-2")).toBe("ready");
    expect(registry.pending()).toEqual([]);
  });

  it("keeps a real cleanup failure visible without blocking an independent connection", async () => {
    const registry = createSessionCleanupRegistry();
    const resources = new Set<string>();
    const cleanupFailure = new Error("native close failed");
    let cleanupAvailable = false;
    const first = createReservedSessionAttempt({
      makeId: () => "session-1",
      wireEvents: async () => () => {},
      cleanup: registry.forScope("tab:ssh-1", "first server"),
      close: async (id) => {
        if (!resources.has(id)) return;
        if (!cleanupAvailable) throw cleanupFailure;
        resources.delete(id);
      },
    });
    await expect(first.open(async (id) => {
      resources.add(id);
      throw new Error("open response lost");
    })).rejects.toThrow("open response lost");

    const second = createReservedSessionAttempt({
      makeId: () => "session-2",
      wireEvents: async () => () => {},
      cleanup: registry.forScope("tab:ssh-2", "second server"),
      close: async (id) => { resources.delete(id); },
    });
    await expect(second.open(async (id) => {
      resources.add(id);
      return id;
    })).resolves.toEqual({ kind: "ready", sessionId: "session-2" });
    expect(registry.pending()).toEqual([{
      sessionId: "session-1", scope: "tab:ssh-1", label: "first server",
      status: { kind: "failed", error: cleanupFailure },
    }]);
    expect(resources).toEqual(new Set(["session-1", "session-2"]));

    cleanupAvailable = true;
    await registry.retry("session-1");
    expect(registry.pending()).toEqual([]);
    expect(resources).toEqual(new Set(["session-2"]));
    expect(second.accepts("session-2")).toBe(true);
  });

  it("rejects a backend id that differs from the canonical reservation", async () => {
    const close = vi.fn();
    const disposeEvents = vi.fn();
    const attempt = createReservedSessionAttempt({
      makeId: () => "reserved-1",
      wireEvents: async () => disposeEvents,
      close,
    });

    await expect(attempt.open(async () => "opened-1")).rejects.toThrow(
      "backend returned a different session id",
    );
    expect(disposeEvents).toHaveBeenCalledOnce();
    expect(attempt.accepts("reserved-1")).toBe(false);
    expect(attempt.accepts("opened-1")).toBe(false);
    expect(close).toHaveBeenCalledWith("reserved-1");
    expect(close).toHaveBeenCalledWith("opened-1");
  });

  it("keeps a newer ready attempt when the superseded open returns late", async () => {
    const firstOpened = deferred<string>();
    const backendStarted = deferred<void>();
    const close = vi.fn();
    let nextId = 0;
    const attempt = createReservedSessionAttempt({
      makeId: () => `reserved-${++nextId}`,
      wireEvents: async () => () => {},
      close,
    });

    const first = attempt.open(() => {
      backendStarted.resolve(undefined);
      return firstOpened.promise;
    });
    await backendStarted.promise;

    await expect(attempt.open(async (id) => id)).resolves.toEqual({
      kind: "ready",
      sessionId: "reserved-2",
    });
    expect(attempt.accepts("reserved-2")).toBe(true);

    firstOpened.resolve("opened-1");
    await expect(first).resolves.toEqual({ kind: "cancelled" });
    expect(close).toHaveBeenCalledWith("opened-1");
    expect(close).not.toHaveBeenCalledWith("reserved-2");
    expect(attempt.accepts("reserved-2")).toBe(true);
  });

  it("does not hide an open error behind a close that never settles", async () => {
    const attempt = createReservedSessionAttempt({
      makeId: () => "reserved-1",
      wireEvents: async () => () => {},
      close: () => new Promise<void>(() => {}),
    });

    await expect(attempt.open(async () => {
      throw new Error("open failed");
    })).rejects.toThrow("open failed");
  });

  it("waits for the old native close before wiring and opening a replacement", async () => {
    const closed = deferred<void>();
    const disposeEvents = vi.fn();
    const wireEvents = vi.fn(async () => disposeEvents);
    const openBackend = vi.fn(async (id: string) => id);
    const close = vi.fn(() => closed.promise);
    let nextId = 0;
    const attempt = createReservedSessionAttempt({
      makeId: () => `reserved-${++nextId}`,
      wireEvents,
      close,
    });

    await attempt.open(openBackend);
    const replacement = attempt.open(openBackend);
    expect(attempt.accepts("reserved-1")).toBe(false);
    expect(disposeEvents).toHaveBeenCalledOnce();
    expect(close).toHaveBeenCalledWith("reserved-1");
    await Promise.resolve();
    expect(wireEvents).toHaveBeenCalledTimes(1);
    expect(openBackend).toHaveBeenCalledTimes(1);

    closed.resolve(undefined);
    await expect(replacement).resolves.toEqual({ kind: "ready", sessionId: "reserved-2" });
    expect(wireEvents).toHaveBeenLastCalledWith("reserved-2");
    expect(openBackend).toHaveBeenLastCalledWith("reserved-2");
  });

  it.each(["cancel", "destroy"] as const)("%s invalidates an attempt waiting for close immediately", async (action) => {
    const closed = deferred<void>();
    const wireEvents = vi.fn(async () => () => {});
    const openBackend = vi.fn(async (id: string) => id);
    const close = vi.fn((id: string) => id === "reserved-1" ? closed.promise : Promise.resolve());
    let nextId = 0;
    const attempt = createReservedSessionAttempt({
      makeId: () => `reserved-${++nextId}`,
      wireEvents,
      close,
    });

    await attempt.open(openBackend);
    const replacement = attempt.open(openBackend);
    expect(attempt.isPending()).toBe(true);
    attempt[action]();
    expect(attempt.isPending()).toBe(false);
    expect(attempt.accepts("reserved-2")).toBe(false);
    expect(close).toHaveBeenCalledWith("reserved-2");

    closed.resolve(undefined);
    await expect(replacement).resolves.toEqual({ kind: "cancelled" });
    expect(wireEvents).toHaveBeenCalledTimes(1);
    expect(openBackend).toHaveBeenCalledTimes(1);
    if (action === "destroy") {
      await expect(attempt.open(openBackend)).resolves.toEqual({ kind: "cancelled" });
    }
  });

  it("waits for every queued close when another open supersedes a waiting attempt", async () => {
    const firstClosed = deferred<void>();
    const secondClosed = deferred<void>();
    const wireEvents = vi.fn(async () => () => {});
    const openBackend = vi.fn(async (id: string) => id);
    let nextId = 0;
    const attempt = createReservedSessionAttempt({
      makeId: () => `reserved-${++nextId}`,
      wireEvents,
      close: (id) => id === "reserved-1" ? firstClosed.promise : secondClosed.promise,
    });

    await attempt.open(openBackend);
    const second = attempt.open(openBackend);
    const third = attempt.open(openBackend);
    firstClosed.resolve(undefined);
    await expect(second).resolves.toEqual({ kind: "cancelled" });
    expect(wireEvents).toHaveBeenCalledTimes(1);
    expect(openBackend).toHaveBeenCalledTimes(1);

    secondClosed.resolve(undefined);
    await expect(third).resolves.toEqual({ kind: "ready", sessionId: "reserved-3" });
    expect(wireEvents).toHaveBeenLastCalledWith("reserved-3");
    expect(openBackend).toHaveBeenLastCalledWith("reserved-3");
  });

  it("returns the open error immediately but waits for its cleanup before retrying", async () => {
    const closed = deferred<void>();
    const wireEvents = vi.fn(async () => () => {});
    const retryBackend = vi.fn(async (id: string) => id);
    let nextId = 0;
    const attempt = createReservedSessionAttempt({
      makeId: () => `reserved-${++nextId}`,
      wireEvents,
      close: () => closed.promise,
    });

    await expect(attempt.open(async () => { throw new Error("response lost"); }))
      .rejects.toThrow("response lost");
    const retry = attempt.open(retryBackend);
    await Promise.resolve();
    expect(wireEvents).toHaveBeenCalledTimes(1);
    expect(retryBackend).not.toHaveBeenCalled();

    closed.resolve(undefined);
    await expect(retry).resolves.toEqual({ kind: "ready", sessionId: "reserved-2" });
  });

  it("includes cleanup of a late result queued while waiting for close", async () => {
    const firstOpened = deferred<string>();
    const backendStarted = deferred<void>();
    const reservedClosed = deferred<void>();
    const lateClosed = deferred<void>();
    const retryBackend = vi.fn(async (id: string) => id);
    let nextId = 0;
    const attempt = createReservedSessionAttempt({
      makeId: () => `reserved-${++nextId}`,
      wireEvents: async () => () => {},
      close: (id) => id === "reserved-1" ? reservedClosed.promise : lateClosed.promise,
    });

    const first = attempt.open(() => {
      backendStarted.resolve(undefined);
      return firstOpened.promise;
    });
    await backendStarted.promise;
    const replacement = attempt.open(retryBackend);
    firstOpened.resolve("opened-1");
    await expect(first).resolves.toEqual({ kind: "cancelled" });
    reservedClosed.resolve(undefined);
    // Let all continuations of the first close run; the later close must still
    // prevent opening, rather than merely delaying it by another microtask.
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    expect(retryBackend).not.toHaveBeenCalled();

    lateClosed.resolve(undefined);
    await expect(replacement).resolves.toEqual({ kind: "ready", sessionId: "reserved-2" });
    expect(attempt.accepts("reserved-2")).toBe(true);
  });

  it("preserves a failed close and retries that same id before opening the port", async () => {
    let nextId = 0;
    let nativeOwner: string | undefined;
    let failClose = true;
    const close = vi.fn(async (id: string) => {
      if (id !== nativeOwner) return;
      if (failClose) throw new Error("native close failed");
      nativeOwner = undefined;
    });
    const openBackend = vi.fn(async (id: string) => {
      if (nativeOwner) throw new Error("port already in use");
      nativeOwner = id;
      return id;
    });
    const attempt = createReservedSessionAttempt({
      makeId: () => `reserved-${++nextId}`,
      wireEvents: async () => () => {},
      close,
    });

    await attempt.open(openBackend);
    await expect(attempt.open(openBackend)).rejects.toThrow("native close failed");
    expect(attempt.isPending()).toBe(false);
    expect(openBackend).toHaveBeenCalledTimes(1);
    expect(nativeOwner).toBe("reserved-1");

    failClose = false;
    await expect(attempt.open(openBackend)).resolves.toEqual({ kind: "ready", sessionId: "reserved-3" });
    expect(close.mock.calls.filter(([id]) => id === "reserved-1")).toHaveLength(2);
    expect(nativeOwner).toBe("reserved-3");
  });

  it("retains a failed close after destroying the pane and cleans it before a new pane opens the port", async () => {
    const registry = createSessionCleanupRegistry();
    const scope = registry.forScope("serial:device-1", "device-1");
    const closeFailure = new Error("native close failed");
    const close = vi.fn().mockRejectedValueOnce(closeFailure).mockResolvedValue(undefined);
    const first = createReservedSessionAttempt({
      makeId: () => "old-session",
      wireEvents: async () => () => {},
      cleanup: scope,
      close,
    });
    await first.open(async (id) => id);
    first.destroy();
    await vi.waitFor(() => expect(registry.pending()[0]?.status).toEqual({ kind: "failed", error: closeFailure }));

    const openBackend = vi.fn(async (id: string) => {
      expect(close).toHaveBeenLastCalledWith("old-session");
      expect(registry.pending()).toEqual([]);
      return id;
    });
    const second = createReservedSessionAttempt({
      makeId: () => "new-session",
      wireEvents: async () => () => {},
      cleanup: registry.forScope("serial:device-1", "device-1"),
      close,
    });
    await expect(second.open(openBackend)).resolves.toEqual({ kind: "ready", sessionId: "new-session" });
    expect(close).toHaveBeenCalledTimes(2);
  });
});
