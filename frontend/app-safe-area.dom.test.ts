import { afterEach, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import App from "./App.svelte";
import * as runtime from "./lib/stores/runtime.svelte.ts";

const { invokeMock, subscribeMock, shellMounted } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  subscribeMock: vi.fn(),
  shellMounted: vi.fn(() => document.getElementById("app")!.classList.contains("status-bar-hidden")),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("./lib/ipc-shim.ts", () => ({ onRuntimeCapabilitiesChanged: subscribeMock }));
vi.mock("./lib/components/AppShell.svelte", () => ({ default: shellMounted }));
vi.mock("./lib/components/ToastStack.svelte", () => ({ default: () => {} }));
vi.mock("./lib/components/WelcomeScreen.svelte", () => ({ default: () => {} }));
vi.mock("./lib/stores/app.svelte.ts", () => ({
  loadProfiles: async () => [],
  loadForwards: async () => [],
}));
vi.mock("./lib/ai/store.svelte.ts", () => ({ settings: () => ({}), loadSettings: vi.fn() }));
vi.mock("./lib/stores/sync.svelte.ts", () => ({
  configurationRevision: () => 0,
  startBackgroundChecks: vi.fn(),
}));
vi.mock("./lib/stores/cli.svelte.ts", () => ({ startBackgroundChecks: vi.fn() }));
vi.mock("./lib/stores/updates.svelte.ts", () => ({ startBackgroundChecks: vi.fn() }));

let component: ReturnType<typeof mount> | undefined;
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.innerHTML = "";
  localStorage.clear();
  vi.unstubAllGlobals();
});

it("applies the host's top-inset policy before mounting the shell, independently of width and keyboard policy", async () => {
  vi.stubGlobal("navigator", { userAgent: "Mozilla/5.0 (Linux; Android 16)", maxTouchPoints: 5 });
  window.innerWidth = 1200;
  localStorage.setItem("rssh.welcome.dismissed", "true");
  const target = document.createElement("div");
  target.id = "app";
  document.body.append(target);
  let finish!: (response: runtime.RuntimeResponse) => void;
  invokeMock.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  component = mount(App, { target });
  flushSync();
  expect(shellMounted).not.toHaveBeenCalled();

  const response = {
    ...runtime.capabilities(),
    statusBarHidden: true,
    terminalKeybar: false,
    terminalPolicy: {
      imageStorageLimitMb: 32,
      imagePixelLimit: 4_000_000,
      outputBacklogLimitBytes: 32 * 1024 * 1024,
      gpuRenderDefault: false,
    },
  };
  finish(response);
  await vi.waitFor(() => {
    flushSync();
    expect(shellMounted).toHaveReturnedWith(true);
  });

  window.innerWidth = 400;
  window.dispatchEvent(new Event("resize"));
  flushSync();
  expect(target.classList.contains("status-bar-hidden")).toBe(true);

  invokeMock.mockResolvedValue({ ...response, statusBarHidden: false, terminalKeybar: true });
  subscribeMock.mock.calls[0][0]();
  await vi.waitFor(() => {
    flushSync();
    expect(target.classList.contains("status-bar-hidden")).toBe(false);
  });
  expect(shellMounted).toHaveBeenCalledTimes(1);

  invokeMock.mockResolvedValue(response);
  subscribeMock.mock.calls[0][0]();
  await vi.waitFor(() => {
    flushSync();
    expect(target.classList.contains("status-bar-hidden")).toBe(true);
  });
  await unmount(component);
  component = undefined;
  expect(target.classList.contains("status-bar-hidden")).toBe(false);
});
