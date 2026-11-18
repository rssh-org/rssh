import { afterEach, expect, it, vi } from "vitest";
import { createReservedSessionAttempt } from "../terminal/reserved-session-attempt.ts";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const stopObservers: Array<() => void> = [];
afterEach(() => {
  for (const stop of stopObservers.splice(0)) stop();
  vi.useRealTimers();
  vi.restoreAllMocks();
  invoke.mockReset();
});

it("keeps the window retry control usable after a terminal pane is destroyed", async () => {
  vi.resetModules();
  const store = await import("./session-cleanup.svelte.ts");
  const error = new Error("close service unavailable");
  const close = vi.fn().mockRejectedValueOnce(error).mockResolvedValue(undefined);
  const pane = createReservedSessionAttempt({
    makeId: () => "session-1",
    wireEvents: async () => () => {},
    cleanup: store.sessionCleanupScope("serial:usb-device-1", "USB device"),
    close,
  });
  await pane.open(async (id) => id);
  pane.destroy();
  await vi.waitFor(() => expect(store.failedSessionCleanups()).toEqual([
    { sessionId: "session-1", label: "USB device", error },
  ]));
  await store.retrySessionCleanup("session-1");
  expect(store.failedSessionCleanups()).toEqual([]);
  expect(close.mock.calls).toEqual([["session-1"], ["session-1"]]);
});

it("removes a failed notice when backend cleanup succeeds without reissuing close", async () => {
  vi.resetModules();
  vi.useFakeTimers();
  const store = await import("./session-cleanup.svelte.ts");
  const close = vi.fn().mockRejectedValue(new Error("temporary AWS failure"));
  const scope = store.sessionCleanupScope("tab:aws", "Production EC2");
  scope.requestClose("session-1", close);
  await expect(scope.wait()).rejects.toThrow("temporary AWS failure");
  store.hideSessionCleanup("session-1");
  invoke.mockResolvedValue(["session-1"]);
  stopObservers.push(store.observeSessionCleanupStatus());
  await vi.advanceTimersByTimeAsync(4999);
  expect(invoke).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(invoke).toHaveBeenCalledWith("session_cleanup_status", { sessionIds: ["session-1"] });
  expect(store.failedSessionCleanups()).toEqual([]);
  expect(store.hiddenSessionCleanupCount()).toBe(0);
  expect(close).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(15_000);
  expect(invoke).toHaveBeenCalledTimes(1);
});

it("keeps the original failure after status errors and stops checking when notices unmount", async () => {
  vi.resetModules();
  vi.useFakeTimers();
  const store = await import("./session-cleanup.svelte.ts");
  const scope = store.sessionCleanupScope("tab:aws", "Production EC2");
  const failure = new Error("AWS close failed");
  const close = vi.fn().mockRejectedValue(failure);
  scope.requestClose("session-1", close);
  await expect(scope.wait()).rejects.toBe(failure);
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  invoke.mockRejectedValue(new Error("IPC unavailable"));
  const stop = store.observeSessionCleanupStatus();
  stopObservers.push(stop);
  await vi.advanceTimersByTimeAsync(5000);
  expect(store.failedSessionCleanups()[0].error).toBe(failure);
  expect(warn).toHaveBeenCalledTimes(1);
  stop();
  await vi.advanceTimersByTimeAsync(15_000);
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(close).toHaveBeenCalledTimes(1);
});

it("ignores unrequested IDs and a status response arriving during a foreground retry", async () => {
  vi.resetModules();
  vi.useFakeTimers();
  const store = await import("./session-cleanup.svelte.ts");
  const scope = store.sessionCleanupScope("tab:aws", "Production EC2");
  let failRetry!: (error: unknown) => void;
  const close = vi.fn().mockRejectedValueOnce(new Error("first failure"))
    .mockImplementationOnce(() => new Promise<void>((_, reject) => { failRetry = reject; }));
  scope.requestClose("session-1", close);
  await expect(scope.wait()).rejects.toThrow("first failure");
  let finishStatus!: (ids: string[]) => void;
  invoke.mockImplementation(() => new Promise<string[]>(resolve => { finishStatus = resolve; }));
  stopObservers.push(store.observeSessionCleanupStatus());
  await vi.advanceTimersByTimeAsync(5000);
  const retry = store.retrySessionCleanup("session-1");
  const other = store.sessionCleanupScope("tab:other", "Other EC2");
  other.requestClose("session-2", async () => { throw new Error("other failure"); });
  await expect(other.wait()).rejects.toThrow("other failure");
  finishStatus(["session-1", "session-2"]);
  await vi.advanceTimersByTimeAsync(0);
  failRetry(new Error("retry failed"));
  await retry;
  expect(store.failedSessionCleanups().map(item => item.sessionId).sort()).toEqual(["session-1", "session-2"]);
});

it("acknowledges only the same failure while retaining manual retry and surfacing a new error", async () => {
  vi.resetModules();
  const store = await import("./session-cleanup.svelte.ts");
  const scope = store.sessionCleanupScope("tab:aws", "Production EC2");
  const close = vi.fn().mockRejectedValueOnce(new Error("failure A"))
    .mockRejectedValueOnce(new Error("failure A")).mockRejectedValueOnce(new Error("failure B"));
  scope.requestClose("session-1", close);
  await expect(scope.wait()).rejects.toThrow("failure A");
  store.hideSessionCleanup("session-1");
  expect(store.sessionCleanupNotices()).toEqual([]);
  expect(store.hiddenSessionCleanupCount()).toBe(1);
  await store.retrySessionCleanup("session-1");
  expect(store.sessionCleanupNotices()).toEqual([]);
  expect(store.failedSessionCleanups()).toHaveLength(1);
  await store.retrySessionCleanup("session-1");
  expect(store.sessionCleanupNotices()[0].error).toEqual(new Error("failure B"));
  expect(store.hiddenSessionCleanupCount()).toBe(0);
  expect(close).toHaveBeenCalledTimes(3);
});

it("checks every failed session in bounded batches even when the first batch fails", async () => {
  vi.resetModules();
  vi.useFakeTimers();
  const store = await import("./session-cleanup.svelte.ts");
  const scope = store.sessionCleanupScope("tab:aws", "Production EC2");
  const close = vi.fn().mockRejectedValue(new Error("AWS close failed"));
  const ids = Array.from({ length: 257 }, () => crypto.randomUUID());
  for (const id of ids) scope.requestClose(id, close);
  await expect(scope.wait()).rejects.toThrow("AWS close failed");
  vi.spyOn(console, "warn").mockImplementation(() => {});
  invoke.mockRejectedValueOnce(new Error("First batch unavailable")).mockResolvedValueOnce([ids[256]]);
  stopObservers.push(store.observeSessionCleanupStatus());
  await vi.advanceTimersByTimeAsync(5000);
  expect(invoke.mock.calls.map(([, args]) => args.sessionIds.length)).toEqual([256, 1]);
  expect(invoke.mock.calls.flatMap(([, args]) => args.sessionIds)).toEqual(ids);
  expect(store.failedSessionCleanups().map(item => item.sessionId)).toEqual(ids.slice(0, 256));
  expect(close).toHaveBeenCalledTimes(257);
});

it("ignores a status response after the notice UI unmounts and resumes with a fresh check", async () => {
  vi.resetModules();
  vi.useFakeTimers();
  const store = await import("./session-cleanup.svelte.ts");
  const scope = store.sessionCleanupScope("tab:aws", "Production EC2");
  scope.requestClose("session-1", async () => { throw new Error("AWS close failed"); });
  await expect(scope.wait()).rejects.toThrow("AWS close failed");
  let finishStatus!: (ids: string[]) => void;
  invoke.mockImplementationOnce(() => new Promise<string[]>(resolve => { finishStatus = resolve; }))
    .mockResolvedValue(["session-1"]);
  const stop = store.observeSessionCleanupStatus();
  stopObservers.push(stop);
  await vi.advanceTimersByTimeAsync(5000);
  stop();
  finishStatus(["session-1"]);
  await vi.advanceTimersByTimeAsync(10_000);
  expect(store.failedSessionCleanups()).toHaveLength(1);
  expect(invoke).toHaveBeenCalledTimes(1);
  stopObservers.push(store.observeSessionCleanupStatus());
  await vi.advanceTimersByTimeAsync(5000);
  expect(store.failedSessionCleanups()).toEqual([]);
  expect(invoke).toHaveBeenCalledTimes(2);
});
