import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import AppShell from "./AppShell.svelte";
import { lifecycle } from "./test-fixtures/TerminalWorkspace.svelte";
import * as app from "../stores/app.svelte.ts";
import * as ai from "../ai/store.svelte.ts";
import * as layout from "../stores/layout.svelte.ts";
import * as plugins from "../plugins/store.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("./TerminalSplitLayout.svelte", async () => import("./test-fixtures/TerminalWorkspace.svelte"));

let component: ReturnType<typeof mount> | undefined;
let stopViewport: (() => void) | undefined;

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

beforeEach(() => {
  invokeMock.mockReset().mockImplementation(async (command: string) => {
    if (command === "sftp_connect_session") return "sftp:compact";
    if (command === "sftp_home") return "/home/test";
    if (command === "get_setting" || command === "ai_settings_get") return null;
    return [];
  });
  lifecycle.mounted = 0;
  lifecycle.destroyed = 0;
  app.closeSettings();
  window.innerWidth = 900;
  stopViewport = layout.observeViewport();
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  stopViewport?.();
  for (const tab of app.workspaceTabs()) if (tab.id !== "home") app.closeTab(tab.id);
  ai.setPosition("right");
  plugins.setSidePosition("right");
  document.body.innerHTML = "";
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("AppShell compact panels", () => {
  it.each([
    ["ai", "left"], ["ai", "right"], ["sftp", "left"], ["sftp", "right"],
  ] as const)("allows the %s split's %s plugin panel to resize using the terminal row's full width", async (panel, side) => {
    vi.spyOn(plugins, "hostSupported").mockReturnValue(true);
    vi.spyOn(plugins, "sidePlugins").mockReturnValue([{
      id: "test", name: "Test plugin", version: "1", description: "", author: "",
      area: "side", preview: "", enabled: true, installed_at: 0, sort_order: 0,
    }]);
    vi.spyOn(plugins, "entryUrl").mockReturnValue(null);
    const bounds = HTMLElement.prototype.getBoundingClientRect;
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function () {
      if (this.classList.contains("content")) return new DOMRect(0, 0, 639, 800);
      if (this.classList.contains("side-panel")) return new DOMRect(0, 400, 280, 400);
      return bounds.call(this);
    });
    await resize(639);
    app.setSidebarPosition("top");
    const tabId = "ssh:compact-plugin-resize";
    app.addTab({ id: tabId, type: "ssh", label: "Plugin resize" });
    app.registerSession({ tabId, sessionId: "ssh:plugin-resize", type: "ssh" });
    plugins.setSidePosition(side);
    plugins.openSide(tabId);
    plugins.setSideWidth(tabId, 280);
    if (panel === "ai") app.openAiPanel(tabId);
    else app.openSftp();
    const target = document.createElement("div");
    document.body.append(target);
    component = mount(AppShell, { target });
    await settle();

    const pluginPanel = target.querySelector(".region-header")!.closest("aside")!;
    const handle = pluginPanel.querySelector(".resize-handle")!;
    handle.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, clientX: 100 }));
    document.dispatchEvent(new MouseEvent("mousemove", { clientX: side === "left" ? 139 : 61 }));
    document.dispatchEvent(new MouseEvent("mouseup"));
    await settle();
    expect(plugins.sideWidth(tabId)).toBe(319);
  });

  it.each(["ai", "sftp"] as const)("fits the %s split above the visual viewport bottom and releases the constraint when inactive", async (panel) => {
    const viewport = Object.assign(new EventTarget(), { offsetTop: 30, height: 500 });
    vi.stubGlobal("visualViewport", viewport);
    const addListener = vi.spyOn(viewport, "addEventListener");
    const removeListener = vi.spyOn(viewport, "removeEventListener");
    const tabId = "ssh:compact-viewport";
    app.addTab({ id: tabId, type: "ssh", label: "Viewport test" });
    app.registerSession({ tabId, sessionId: "ssh:viewport", type: "ssh" });
    const target = document.createElement("div");
    document.body.append(target);
    component = mount(AppShell, { target });
    await settle();

    const content = target.querySelector<HTMLElement>(".content")!;
    let contentTop = 40;
    const bounds = vi.spyOn(content, "getBoundingClientRect")
      .mockImplementation(() => new DOMRect(0, contentTop, 400, 700));
    const openPanel = () => panel === "ai" ? app.openAiPanel(tabId) : app.openSftp();
    const closePanel = () => panel === "ai" ? ai.closePanel(tabId) : app.closeSftp();
    await resize(400);
    app.setSidebarPosition("left");
    await settle();
    addListener.mockClear();
    openPanel();
    await settle();
    expect(content.style.maxHeight).toBe("490px");
    const sync = addListener.mock.calls.find(([type]) => type === "resize")![1];
    expect(addListener).toHaveBeenCalledWith("scroll", sync);

    viewport.height = 320;
    viewport.dispatchEvent(new Event("resize"));
    await settle();
    expect(content.style.maxHeight).toBe("310px");
    viewport.offsetTop = 45;
    viewport.dispatchEvent(new Event("scroll"));
    await settle();
    expect(content.style.maxHeight).toBe("325px");

    contentTop = 80;
    app.setSidebarPosition("top");
    await settle();
    expect(content.style.maxHeight).toBe("285px");
    const currentSync = addListener.mock.calls.filter(([type]) => type === "resize").at(-1)![1];

    await closePanel();
    await settle();
    expect(content.style.maxHeight).toBe("");
    expect(removeListener).toHaveBeenCalledWith("resize", currentSync);
    expect(removeListener).toHaveBeenCalledWith("scroll", currentSync);
    bounds.mockClear();
    viewport.dispatchEvent(new Event("resize"));
    viewport.dispatchEvent(new Event("scroll"));
    await settle();
    expect(bounds).not.toHaveBeenCalled();
    expect(content.style.maxHeight).toBe("");

    addListener.mockClear();
    openPanel();
    await settle();
    expect(content.style.maxHeight).toBe("285px");
    const reopenedSync = addListener.mock.calls.find(([type]) => type === "resize")![1];
    await resize(640);
    expect(content.style.maxHeight).toBe("");
    expect(removeListener).toHaveBeenCalledWith("resize", reopenedSync);
    expect(removeListener).toHaveBeenCalledWith("scroll", reopenedSync);
    bounds.mockClear();
    viewport.dispatchEvent(new Event("resize"));
    viewport.dispatchEvent(new Event("scroll"));
    await settle();
    expect(bounds).not.toHaveBeenCalled();
    expect(content.style.maxHeight).toBe("");
  });

  it.each(["left", "right"] as const)("keeps the terminal mounted while switching %s AI and SFTP across the breakpoint", async (position) => {
    const tabId = "ssh:compact-panels";
    app.addTab({ id: tabId, type: "ssh", label: "Panel test" });
    app.registerSession({ tabId, sessionId: "ssh:compact", type: "ssh" });
    ai.setPosition(position);
    app.openAiPanel(tabId);
    app.openSftp();
    const target = document.createElement("div");
    document.body.append(target);
    component = mount(AppShell, { target });
    await vi.waitFor(() => expect(target.querySelector(".sftp")).not.toBeNull());
    const terminal = target.querySelector('[data-testid="terminal-workspace"]');
    expect(terminal).not.toBeNull();

    await resize(640);
    expect(app.sftpOpen()).toBe(true);
    await resize(639);
    expect(ai.isOpen(tabId)).toBe(true);
    expect(app.sftpOpen()).toBe(false);
    expect(target.querySelector(".sftp")).toBeNull();
    expect(invokeMock).toHaveBeenCalledWith("sftp_close", { sftpId: "sftp:compact" });

    app.openSftp();
    await settle();
    expect(ai.isOpen(tabId)).toBe(false);
    expect(app.sftpOpen()).toBe(true);
    app.openAiPanel(tabId);
    await settle();
    expect(ai.isOpen(tabId)).toBe(true);
    expect(app.sftpOpen()).toBe(false);

    await resize(900);
    expect(app.sftpOpen()).toBe(false);
    expect(ai.position()).toBe(position);
    expect(target.querySelector('[data-testid="terminal-workspace"]')).toBe(terminal);
    expect(lifecycle).toEqual({ mounted: 1, destroyed: 0 });
  });
});
