import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { paletteById } from "./lib/themes/palettes.ts";

const shellMounted = vi.hoisted(() => vi.fn(() => ({
  background: document.documentElement.style.getPropertyValue("--bg"),
  shape: document.documentElement.dataset.shape,
  density: document.documentElement.dataset.density,
})));
vi.mock("./lib/components/AppShell.svelte", () => ({ default: shellMounted }));
vi.mock("./lib/components/ToastStack.svelte", () => ({ default: () => {} }));
vi.mock("./lib/components/WelcomeScreen.svelte", () => ({ default: () => {} }));
vi.mock("./lib/stores/app.svelte.ts", () => ({
  loadProfiles: async () => [],
  loadForwards: async () => [],
  loadCopyOnSelect: vi.fn(),
  loadRightClickAction: vi.fn(),
  loadConfirmCloseTab: vi.fn(),
  loadSplitOpensHome: vi.fn(),
  loadTabMru: vi.fn(),
  loadCtrlRightClickMenu: vi.fn(),
}));
vi.mock("./lib/stores/transfers.svelte.ts", () => ({ loadMaxConcurrent: vi.fn() }));
vi.mock("./lib/ai/store.svelte.ts", () => ({ settings: () => ({}), loadSettings: vi.fn() }));
vi.mock("./lib/stores/sync.svelte.ts", () => ({
  configurationRevision: () => 0,
  startBackgroundChecks: vi.fn(),
}));
vi.mock("./lib/stores/cli.svelte.ts", () => ({ startBackgroundChecks: vi.fn() }));
vi.mock("./lib/stores/updates.svelte.ts", () => ({ startBackgroundChecks: vi.fn() }));
vi.mock("./lib/local-drop.ts", () => ({ installLocalFileDropNavigationGuard: vi.fn() }));
vi.mock("./lib/input.ts", () => ({
  installInputTracking: () => () => {},
  isTouchContextMenu: () => false,
}));

type Request = { id: number; cmd: string; args: { key?: string } };

// Exercise the real IPC shim: a socket that has not opened or replied leaves
// its requests pending, unlike a rejected invoke mock.
class Socket {
  static OPEN = 1;
  static current: Socket;
  readyState = 0;
  requests: Request[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor() { Socket.current = this; }
  send(data: string) { this.requests.push(JSON.parse(data)); }
  open() { this.readyState = Socket.OPEN; this.onopen?.(); }
  reply(request: Request, result: unknown, ok = true) {
    this.onmessage?.({ data: JSON.stringify({
      type: "response", id: request.id, ok, [ok ? "result" : "error"]: result,
    }) });
  }
}

const capabilities = {
  statusBarHidden: false,
  terminalPolicy: {
    imageStorageLimitMb: 128, imagePixelLimit: 16_000_000,
    outputBacklogLimitBytes: 128 * 1024 * 1024, gpuRenderDefault: false,
  },
};
let component: ReturnType<typeof import("svelte").mount> | undefined;
let flushSync: typeof import("svelte").flushSync;
let unmount: typeof import("svelte").unmount;

beforeEach(async () => {
  vi.resetModules();
  ({ flushSync, unmount } = await import("svelte"));
  shellMounted.mockClear();
  document.body.innerHTML = '<div id="app"></div>';
  document.documentElement.removeAttribute("style");
  delete document.documentElement.dataset.shape;
  delete document.documentElement.dataset.density;
  localStorage.setItem("rssh.welcome.dismissed", "true");
  window.history.replaceState(null, "", "/?rsshPort=5555&rsshToken=startup-test");
  vi.stubGlobal("WebSocket", Socket);
});

afterEach(async () => {
  Socket.current?.onclose?.();
  if (component) await unmount(component);
  component = undefined;
  document.body.innerHTML = "";
  localStorage.clear();
  for (const key of ["__TAURI_INTERNALS__", "__TAURI_EVENT_PLUGIN_INTERNALS__", "__RSSH_IPC_SHIM__"]) {
    delete (window as unknown as Record<string, unknown>)[key];
  }
  vi.unstubAllGlobals();
});

async function start() {
  component = (await import("./main.ts")).default;
  flushSync();
  return Socket.current;
}

function replyToSettings(socket: Socket, ok = true) {
  const saved: Record<string, string> = {
    "theme.palette": "light-soft", "theme.shape": "flat", "theme.density": "comfortable",
  };
  for (const request of socket.requests.filter((request) => request.cmd === "get_setting")) {
    socket.reply(request, ok ? saved[request.args.key!] ?? null : "settings unavailable", ok);
  }
}

it("keeps startup visible while IPC connects and applies the saved theme before mounting the shell", async () => {
  const socket = await start();
  expect(document.querySelector('[role="status"]')).not.toBeNull();
  expect(shellMounted).not.toHaveBeenCalled();
  socket.open();
  socket.reply(socket.requests.find((request) => request.cmd === "get_runtime_capabilities")!, capabilities);
  const runtime = await import("./lib/stores/runtime.svelte.ts");
  await vi.waitFor(() => {
    flushSync();
    expect(runtime.loaded()).toBe(true);
  });
  expect(document.querySelector('[role="status"]')).not.toBeNull();
  expect(shellMounted).not.toHaveBeenCalled();

  replyToSettings(socket);
  await vi.waitFor(() => {
    flushSync();
    expect(shellMounted).toHaveReturnedWith({
      background: paletteById("light-soft").ui.bg, shape: "flat", density: "comfortable",
    });
  });
  expect(shellMounted).toHaveBeenCalledTimes(1);
});

it.each(["before", "after"])("preserves runtime errors and reuses the theme read when it resolves %s retry", async (arrival) => {
  const socket = await start();
  socket.open();
  socket.reply(socket.requests.find((request) => request.cmd === "get_runtime_capabilities")!, "runtime unavailable", false);
  await vi.waitFor(() => {
    flushSync();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("runtime unavailable");
  });
  const settingsCount = socket.requests.filter((request) => request.cmd === "get_setting").length;
  if (arrival === "before") {
    replyToSettings(socket);
    await vi.waitFor(() => {
      flushSync();
      expect(document.documentElement.style.getPropertyValue("--bg")).toBe(paletteById("light-soft").ui.bg);
    });
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("runtime unavailable");
    expect(shellMounted).not.toHaveBeenCalled();
  }
  document.querySelector<HTMLButtonElement>(".startup button")!.click();
  flushSync();
  const retries = socket.requests.filter((request) => request.cmd === "get_runtime_capabilities");
  expect(retries).toHaveLength(2);
  expect(socket.requests.filter((request) => request.cmd === "get_setting")).toHaveLength(settingsCount);
  socket.reply(retries[1], capabilities);
  if (arrival === "after") replyToSettings(socket);
  await vi.waitFor(() => {
    flushSync();
    expect(shellMounted).toHaveBeenCalledTimes(1);
  });
});

it("mounts with the default theme when settings reads fail", async () => {
  const socket = await start();
  socket.open();
  replyToSettings(socket, false);
  socket.reply(socket.requests.find((request) => request.cmd === "get_runtime_capabilities")!, capabilities);
  await vi.waitFor(() => {
    flushSync();
    expect(shellMounted).toHaveReturnedWith({
      background: paletteById("dark-neumorphism").ui.bg, shape: "neumorphism", density: "cozy",
    });
  });
});

it("shows the startup error when the IPC connection closes before opening", async () => {
  const socket = await start();
  socket.onclose?.();
  await vi.waitFor(() => {
    flushSync();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("connection closed");
  });
  expect(shellMounted).not.toHaveBeenCalled();
});
