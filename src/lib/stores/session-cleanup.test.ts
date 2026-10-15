import { expect, it, vi } from "vitest";
import { createReservedSessionAttempt } from "../terminal/reserved-session-attempt.ts";

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
