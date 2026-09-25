import { afterEach, expect, it, vi } from "vitest";

afterEach(() => { vi.useRealTimers(); });

it("keeps cleanup failures in the toast stack until retry succeeds, independently of toast expiry", async () => {
  vi.resetModules();
  vi.useFakeTimers();
  const { render } = await import("svelte/server");
  const { default: ToastStack } = await import("./ToastStack.svelte");
  const { toast } = await import("../stores/toast.svelte.ts");
  const cleanup = await import("../stores/session-cleanup.svelte.ts");
  const { setLocale } = await import("../i18n/index.svelte.ts");
  setLocale("en");
  let closeAvailable = false;
  const scope = cleanup.sessionCleanupScope("tab:ssh-1", "SSH server");
  const failure = new Error("close service unavailable");
  scope.requestClose("session-1", async () => {
    if (!closeAvailable) throw failure;
  });
  await expect(scope.wait()).rejects.toBe(failure);
  toast.error("original connection error");

  const initial = render(ToastStack).body;
  expect(initial).toContain("original connection error");
  expect(initial).toContain("Connection could not close: SSH server");
  expect(initial).toContain("close service unavailable");
  expect(initial).toContain("Retry");

  vi.advanceTimersByTime(4000);
  const afterExpiry = render(ToastStack).body;
  expect(afterExpiry).not.toContain("original connection error");
  expect(afterExpiry).toContain("close service unavailable");
  closeAvailable = true;
  await cleanup.retrySessionCleanup("session-1");
  expect(render(ToastStack).body).not.toContain("close service unavailable");
});
