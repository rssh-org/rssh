import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import TerminalPane from "./TerminalPane.svelte";
import * as app from "../stores/app.svelte.ts";
import * as layout from "../stores/layout.svelte.ts";
import * as runtime from "../stores/runtime.svelte.ts";
import { setLocale } from "../i18n/index.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn());
const subscribeMock = vi.hoisted(() => vi.fn());
const loadWaveMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("../ipc-shim.ts", () => ({ onRuntimeCapabilitiesChanged: subscribeMock }));
vi.mock("../terminal/environment-wave.ts", async (importOriginal) => ({
  ...await importOriginal<typeof import("../terminal/environment-wave.ts")>(),
  loadTerminalWaveColor: loadWaveMock,
}));
vi.mock("@xterm/xterm", () => ({ Terminal: vi.fn() }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: vi.fn() }));
vi.mock("@xterm/addon-webgl", () => ({ WebglAddon: vi.fn() }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: vi.fn() }));
vi.mock("@xterm/addon-unicode11", () => ({ Unicode11Addon: vi.fn() }));
vi.mock("@xterm/addon-image", () => ({ ImageAddon: vi.fn() }));

const tabId = "ssh:keybar-platform";
let component: ReturnType<typeof mount> | undefined;
let stopViewport: (() => void) | undefined;
let finishWaveLookup: (color: null) => void;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

async function resize(width: number) {
  window.innerWidth = width;
  window.dispatchEvent(new Event("resize"));
  await settle();
}

async function render(terminalKeybar: boolean) {
  invokeMock.mockImplementation(async (command: string) => {
    if (command === "get_runtime_capabilities") return {
      ...runtime.capabilities(),
      terminalKeybar,
      terminalPolicy: {
        imageStorageLimitMb: 128,
        imagePixelLimit: 16_000_000,
        outputBacklogLimitBytes: 8_000_000,
        gpuRenderDefault: false,
      },
    };
    return null;
  });
  if (runtime.loaded()) subscribeMock.mock.calls[0][0]();
  else await runtime.load();
  await vi.waitFor(() => expect(runtime.capabilities().terminalKeybar).toBe(terminalKeybar));

  app.addTab({ id: tabId, type: "ssh", label: "Terminal" });
  const target = document.createElement("div");
  document.body.append(target);
  component = mount(TerminalPane, { target, props: { tabId, tabType: "ssh", meta: {} } });
  await settle();
  return target;
}

beforeEach(() => {
  invokeMock.mockReset();
  // Keep xterm initialization pending while testing the actual pane's controls.
  loadWaveMock.mockReset().mockReturnValue(new Promise<null>((resolve) => { finishWaveLookup = resolve; }));
  setLocale("en");
  window.innerWidth = 1024;
  stopViewport = layout.observeViewport();
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  finishWaveLookup(null);
  await settle();
  stopViewport?.();
  stopViewport = undefined;
  app.closeTab(tabId);
  document.body.innerHTML = "";
});

describe("terminal keybar platform capability", () => {
  it("keeps the mobile keybar and its open extension panel across width changes", async () => {
    const target = await render(true);
    const keybar = target.querySelector(".keybar");
    expect(keybar).not.toBeNull();
    target.querySelector<HTMLButtonElement>('[aria-label="More keys"]')!.click();
    await settle();
    const extension = target.querySelector(".ext-panel");
    expect(extension).not.toBeNull();

    for (const width of [400, 1200]) {
      await resize(width);
      expect(target.querySelector(".keybar")).toBe(keybar);
      expect(target.querySelector(".ext-panel")).toBe(extension);
    }

    target.querySelector<HTMLButtonElement>('[aria-label="More keys"]')!.click();
    await settle();
    expect(target.querySelector(".ext-panel")).toBeNull();
  });

  it("keeps the desktop keybar hidden in narrow and wide windows", async () => {
    const target = await render(false);
    for (const width of [1024, 400, 1200]) {
      await resize(width);
      expect(target.querySelector(".keybar")).toBeNull();
    }
  });

  it("shows the mobile keybar only for the active pane", async () => {
    await resize(400);
    const target = await render(true);
    expect(target.querySelector(".keybar")).not.toBeNull();

    app.setActiveTab("home");
    await settle();
    expect(target.querySelector(".keybar")).toBeNull();

    app.setActiveTab(tabId);
    await settle();
    expect(target.querySelector(".keybar")).not.toBeNull();
  });
});
