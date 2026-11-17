import { afterEach, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
let component: ReturnType<typeof mount> | undefined;
async function settle() { flushSync(); await tick(); flushSync(); }

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

it("lets users hide and reopen cleanup errors, then removes them after backend confirmation", async () => {
  vi.useFakeTimers();
  const { default: ToastStack } = await import("./ToastStack.svelte");
  const cleanup = await import("../stores/session-cleanup.svelte.ts");
  const { setLocale } = await import("../i18n/index.svelte.ts");
  setLocale("en");
  invoke.mockResolvedValue(["session-1"]);
  component = mount(ToastStack, { target: document.body });
  const scope = cleanup.sessionCleanupScope("tab:aws", "Production EC2");
  const close = vi.fn().mockRejectedValue(new Error("AWS close failed"));
  scope.requestClose("session-1", close);
  await expect(scope.wait()).rejects.toThrow("AWS close failed");
  await settle();
  expect(document.querySelectorAll('[role="alert"]')).toHaveLength(1);
  document.querySelector<HTMLButtonElement>('[aria-label="Hide this notice; keep cleanup pending"]')!.click();
  await settle();
  expect(document.querySelector('[role="alert"]')).toBeNull();
  expect(cleanup.failedSessionCleanups()).toHaveLength(1);
  const pending = [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Connections awaiting cleanup: 1"))!;
  pending.click();
  await settle();
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("AWS close failed");
  expect(document.body.textContent).toContain("Retry");
  await vi.advanceTimersByTimeAsync(5000);
  await settle();
  expect(cleanup.failedSessionCleanups()).toEqual([]);
  expect(document.querySelector(".cleanup-notice")).toBeNull();
  expect(document.body.textContent).not.toContain("Connections awaiting cleanup");
  expect(close).toHaveBeenCalledTimes(1);
  await unmount(component);
  component = undefined;
  await vi.advanceTimersByTimeAsync(10_000);
  expect(invoke).toHaveBeenCalledTimes(1);
});
