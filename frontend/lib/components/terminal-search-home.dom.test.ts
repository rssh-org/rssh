import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import TerminalSplitLayout from "./TerminalSplitLayout.svelte";
import * as app from "../stores/app.svelte.ts";
import * as runtime from "../stores/runtime.svelte.ts";
import { canHandleBack, requestBack } from "../stores/back.svelte.ts";
import { setLocale } from "../i18n/index.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn());
const loadWaveMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
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

const rootId = "local:search-home";
const homeId = "home:search-home";
let component: ReturnType<typeof mount> | undefined;
let target: HTMLDivElement;
let finishWaveLookup: (color: null) => void;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

beforeEach(async () => {
  invokeMock.mockReset().mockImplementation(async (command: string) => {
    if (command === "get_runtime_capabilities") return {
      ...runtime.capabilities(),
      terminalPolicy: { imageStorageLimitMb: 128, imagePixelLimit: 16_000_000,
        outputBacklogLimitBytes: 8_000_000, gpuRenderDefault: false },
    };
    if (command === "list_profiles") return [{
      id: "profile:chosen", name: "Chosen server", host: "example.com", port: 22,
      credential_id: "credential", group_id: null,
    }];
    if (command.startsWith("list_")) return [];
    return null;
  });
  // Keep terminal rendering/transport initialization pending; the search UI,
  // Home, split layout and their shared application/back stores remain real.
  loadWaveMock.mockReset().mockReturnValue(new Promise<null>((resolve) => { finishWaveLookup = resolve; }));
  setLocale("en");
  await runtime.load();
  app.addTab({ id: rootId, type: "local", label: "Terminal" });
  app.addPane(rootId, "right", { id: homeId, type: "home", label: "" });
  app.setActivePane(rootId);
  target = document.createElement("div");
  document.body.append(target);
  component = mount(TerminalSplitLayout, {
    target,
    props: {
      get layout() { return app.layoutForWorkspace(rootId)!; },
      get activePaneId() { return app.activePaneId(); },
      onActivate: app.setActivePane,
      onResize: (path, ratio) => app.resizeLayoutPath(rootId, path, ratio),
      onClose: app.closePane,
      onContextMenu: vi.fn(),
      onInitialConnectionFailure: vi.fn(() => false),
    },
  });
  await vi.waitFor(() => expect(target.querySelector(".card-name")?.textContent).toBe("Chosen server"));
  app.requestSearch(rootId);
  await settle();
  await vi.waitFor(() => expect(document.activeElement).toBe(target.querySelector(".search-bar input")));
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  finishWaveLookup(null);
  await settle();
  app.closeTab(rootId);
  document.body.innerHTML = "";
});

describe("terminal search beside Home", () => {
  it("keeps Home arrow and Enter navigation working while a sibling search stays open", async () => {
    expect(canHandleBack()).toBe(true);
    const search = target.querySelector(".search-bar")!;
    target.querySelector(".home")!.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    await settle();
    expect(app.activePaneId()).toBe(homeId);
    expect(canHandleBack()).toBe(false);

    const arrow = new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true });
    document.body.dispatchEvent(arrow);
    await settle();
    expect(arrow.defaultPrevented).toBe(true);
    expect(target.querySelector(".card-btn.selected")).not.toBeNull();
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await settle();

    expect(app.activeTab()).toMatchObject({ type: "ssh", label: "Chosen server", paneOf: rootId });
    expect(app.paneIdsForWorkspace(rootId)).toHaveLength(2);
    expect(target.querySelector(".search-bar")).toBe(search);
  });

  it("preserves search until its terminal is focused and then lets Back close it", async () => {
    expect(canHandleBack()).toBe(true);
    app.setActivePane(homeId);
    await settle();
    expect(requestBack()).toBe(false);
    expect(target.querySelector(".search-bar")).not.toBeNull();

    app.setActivePane(rootId);
    await settle();
    expect(canHandleBack()).toBe(true);
    expect(requestBack()).toBe(true);
    await settle();
    expect(target.querySelector(".search-bar")).toBeNull();
    expect(canHandleBack()).toBe(false);
  });
});
