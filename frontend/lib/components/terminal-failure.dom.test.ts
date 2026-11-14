import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import AppShell from "./AppShell.svelte";
import * as app from "../stores/app.svelte.ts";
import * as layout from "../stores/layout.svelte.ts";
import * as runtime from "../stores/runtime.svelte.ts";
import { setLocale, t } from "../i18n/index.svelte.ts";
import { installInputTracking } from "../input.ts";

const mocks = vi.hoisted(() => {
  const disposable = () => ({ dispose() {} });
  class Terminal {
    static instances: Terminal[] = [];
    cols = 80;
    rows = 24;
    unicode = { activeVersion: "" };
    modes = { bracketedPasteMode: false };
    parser = { registerOscHandler: disposable, registerEscHandler: disposable };
    buffer = {
      active: { type: "normal", baseY: 0, viewportY: 0, cursorX: 0, cursorY: 0, getLine: () => undefined },
      normal: { cursorX: 0, cursorY: 0 },
      onBufferChange: disposable,
    };
    output!: HTMLPreElement;
    disposed = false;
    private dataListeners = new Set<(data: string) => void>();
    private titleListeners = new Set<(title: string) => void>();
    constructor(public options: Record<string, unknown>) { Terminal.instances.push(this); }
    open(host: HTMLElement) { this.output = document.createElement("pre"); host.append(this.output); }
    write(data: string | Uint8Array, callback?: () => void) {
      this.output.textContent += typeof data === "string" ? data : new TextDecoder().decode(data);
      callback?.();
    }
    onData(listener: (data: string) => void) {
      this.dataListeners.add(listener);
      return { dispose: () => this.dataListeners.delete(listener) };
    }
    input(data: string) { for (const listener of [...this.dataListeners]) listener(data); }
    onTitleChange(listener: (title: string) => void) {
      this.titleListeners.add(listener);
      return { dispose: () => this.titleListeners.delete(listener) };
    }
    setTitle(title: string) { for (const listener of this.titleListeners) listener(title); }
    onResize = disposable;
    onWriteParsed = disposable;
    onScroll = disposable;
    onRender = disposable;
    onCursorMove = disposable;
    onLineFeed = disposable;
    loadAddon() {}
    attachCustomKeyEventHandler() {}
    getSelection() { return ""; }
    focus() {}
    dispose() { this.disposed = true; this.dataListeners.clear(); this.titleListeners.clear(); }
  }
  class Addon {
    fit() {}
    proposeDimensions() { return { cols: 80, rows: 24 }; }
    dispose() {}
  }
  return { Terminal, Addon, invoke: vi.fn(), listen: vi.fn() };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("@xterm/xterm", () => ({ Terminal: mocks.Terminal }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: mocks.Addon }));
vi.mock("@xterm/addon-webgl", () => ({ WebglAddon: mocks.Addon }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: mocks.Addon }));
vi.mock("@xterm/addon-unicode11", () => ({ Unicode11Addon: mocks.Addon }));
vi.mock("@xterm/addon-image", () => ({ ImageAddon: mocks.Addon }));

const rootId = "ssh:failure-root";
let component: ReturnType<typeof mount> | undefined;
let target: HTMLDivElement;
let stopViewport: (() => void) | undefined;
let stopInputTracking: (() => void) | undefined;
let rejectConnection: ((error: Error) => void) | undefined;
let firstPaneAttempt: string | undefined;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

beforeEach(async () => {
  stopInputTracking = installInputTracking();
  mocks.Terminal.instances = [];
  rejectConnection = undefined;
  firstPaneAttempt = undefined;
  mocks.listen.mockReset().mockResolvedValue(() => {});
  mocks.invoke.mockReset().mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    if (command === "get_runtime_capabilities") return {
      ...runtime.capabilities(),
      terminalPolicy: { imageStorageLimitMb: 128, imagePixelLimit: 16_000_000,
        outputBacklogLimitBytes: 8_000_000, gpuRenderDefault: false },
    };
    if (command === "list_profiles") return [{
      id: "profile:unavailable", name: "Unavailable server", host: "unavailable.example", port: 22,
      credential_id: "credential", group_id: null,
    }];
    if (command === "get_profile") return { group_id: null };
    if (command.startsWith("list_")) return [];
    if (command === "get_setting") return args?.key === "command_block_bar" ? "false" : null;
    if (command === "ssh_connect") {
      if (args?.logSessionId !== rootId && !firstPaneAttempt) {
        firstPaneAttempt = args?.sessionId as string;
        return new Promise((_resolve, reject) => { rejectConnection = reject; });
      }
      return args?.sessionId;
    }
    return null;
  });
  setLocale("en");
  app.closeSettings();
  app.closeDownloads();
  app.closeSnippetPicker();
  window.innerWidth = 1200;
  stopViewport = layout.observeViewport();
  await runtime.load();
  app.addTab({ id: rootId, type: "ssh", label: "Existing session", meta: { profileId: "profile:existing" } });
  target = document.createElement("div");
  document.body.append(target);
  component = mount(AppShell, { target });
  await vi.waitFor(() => expect(app.sessionIdForTab(rootId)).toBeTruthy());
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  stopViewport?.();
  stopViewport = undefined;
  stopInputTracking?.();
  stopInputTracking = undefined;
  window.getSelection()?.removeAllRanges();
  for (const tab of app.workspaceTabs()) app.closeTab(tab.id);
  await settle();
  document.body.innerHTML = "";
});

describe("native terminal selection handle context menus", () => {
  it.each(["terminal output", "pane"])("keeps handle drags on %s native, then accepts mouse and keyboard menus", async (area) => {
    await app.setRightClickAction("menu");
    const output = mocks.Terminal.instances[0].output;
    const pane = output.closest<HTMLElement>(".pane")!;
    const menuTarget = area === "terminal output" ? output : pane;
    output.textContent = "selected terminal output";

    output.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerType: "touch" }));
    output.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, pointerType: "touch" }));
    const selection = window.getSelection()!;
    const range = document.createRange();
    range.setStart(output.firstChild!, 0);
    range.setEnd(output.firstChild!, 8);
    selection.addRange(range);
    expect(selection.toString()).toBe("selected");
    const initialMenu = new PointerEvent("contextmenu", {
      bubbles: true, cancelable: true, pointerType: "touch", button: 2,
    });
    output.dispatchEvent(initialMenu);
    await settle();
    expect(initialMenu.defaultPrevented).toBe(false);
    expect(target.querySelector(".ctx-menu")).toBeNull();

    // Android's native handles extend the DOM selection without a pointerdown
    // in the page. Older WebViews then dispatch a mouse-tagged contextmenu.
    for (const [index, sourceCapabilities] of [undefined, null, { firesTouchEvents: false }].entries()) {
      range.setEnd(output.firstChild!, 10 + index * 5);
      const selectedText = selection.toString();
      expect(selectedText).toBe(output.textContent.slice(0, 10 + index * 5));
      const event = new PointerEvent("contextmenu", {
        bubbles: true, cancelable: true, pointerType: "mouse", button: 2,
      });
      if (sourceCapabilities !== undefined) {
        Object.defineProperty(event, "sourceCapabilities", { value: sourceCapabilities });
      }
      menuTarget.dispatchEvent(event);
      await settle();
      expect(target.querySelector(".ctx-menu")).toBeNull();
      expect(event.defaultPrevented).toBe(false);
      expect(selection.toString()).toBe(selectedText);
    }

    // A real mouse supplies its own pointerdown and must regain the pane menu.
    menuTarget.dispatchEvent(new PointerEvent("pointerdown", {
      bubbles: true, pointerType: "mouse", button: 2,
    }));
    const mouseMenu = new PointerEvent("contextmenu", {
      bubbles: true, cancelable: true, pointerType: "mouse", button: 2,
    });
    menuTarget.dispatchEvent(mouseMenu);
    await settle();
    expect(target.querySelector(".ctx-menu")).not.toBeNull();
    expect(mouseMenu.defaultPrevented).toBe(true);
    target.querySelector<HTMLElement>(".ctx-backdrop")!.click();
    await settle();

    // The keyboard context-menu key also supersedes the previous touch source.
    output.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerType: "touch" }));
    menuTarget.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "ContextMenu" }));
    const keyboardMenu = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    menuTarget.dispatchEvent(keyboardMenu);
    await settle();
    expect(target.querySelector(".ctx-menu")).not.toBeNull();
    expect(keyboardMenu.defaultPrevented).toBe(true);
  });
});

describe("failed split connections", () => {
  it.each(["Home selection", "cloned session"])("keeps the pane and retries after a failed %s", async (entry) => {
    await app.setSplitOpensHome(entry === "Home selection");
    const rootSession = app.sessionIdForTab(rootId);
    const rootTerminal = mocks.Terminal.instances[0];
    const rootPane = target.querySelector<HTMLElement>(".terminal-split-layout > .pane")!;
    rootPane.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, button: 2 }));
    await settle();
    const splitButton = [...target.querySelectorAll<HTMLButtonElement>(".ctx-item")]
      .find((button) => button.querySelector(".ctx-label")?.textContent === t("tab.context.split"))!;
    expect(splitButton).toBeTruthy();
    splitButton.click();
    await settle();

    if (entry === "Home selection") {
      await vi.waitFor(() => expect(target.querySelector(".terminal-split-layout .card-btn")).not.toBeNull());
      target.querySelector<HTMLButtonElement>(".terminal-split-layout .card-btn")!.click();
    }
    await vi.waitFor(() => expect(rejectConnection).toBeTypeOf("function"));
    const paneId = app.paneIdsForWorkspace(rootId).find((id) => id !== rootId)!;
    app.resizeLayoutPath(rootId, [], 0.37);
    await settle();
    const splitLayout = JSON.stringify(app.layoutForWorkspace(rootId));
    const failedTerminal = mocks.Terminal.instances[1];
    const failedPane = failedTerminal.output.closest(".pane");

    rejectConnection!(new Error("Connection refused by test server"));
    await vi.waitFor(() => expect(failedTerminal.output.textContent).toContain("Connection refused by test server"));
    await settle();

    expect(app.tabs().some((tab) => tab.id === paneId)).toBe(true);
    expect(app.terminalConnectionStatus(paneId)).toBe("disconnected");
    expect(app.activePaneId()).toBe(paneId);
    expect(JSON.stringify(app.layoutForWorkspace(rootId))).toBe(splitLayout);
    expect(app.paneIdsForWorkspace(rootId)).toEqual([rootId, paneId]);
    expect(failedTerminal.output.isConnected).toBe(true);
    expect(failedPane?.textContent).toContain("Connection refused by test server");
    expect(failedPane?.textContent).toContain("Press any key to reconnect");
    expect(app.sessionIdForTab(rootId)).toBe(rootSession);
    expect(rootTerminal.disposed).toBe(false);
    expect(rootTerminal.output.closest(".pane")).toBe(rootPane);

    // Exercise TerminalPane's existing onData retry, with the real session
    // attempt/lifecycle helpers and application stores still in the path.
    failedTerminal.input("x");
    await vi.waitFor(() => expect(app.terminalConnectionStatus(paneId)).toBe("connected"));
    expect(app.sessionIdForTab(paneId)).toBeTruthy();
    expect(app.sessionIdForTab(paneId)).not.toBe(firstPaneAttempt);
    failedTerminal.setTitle("Recovered remote shell");
    expect(app.terminalTitle(paneId)).toBe("Recovered remote shell");
    expect(JSON.stringify(app.layoutForWorkspace(rootId))).toBe(splitLayout);
    expect(failedTerminal.output.closest(".pane")).toBe(failedPane);
    expect(app.sessionIdForTab(rootId)).toBe(rootSession);
    expect(mocks.invoke.mock.calls.filter(([command, args]) =>
      command === "ssh_connect" && args.logSessionId === rootId)).toHaveLength(1);
    expect(mocks.invoke.mock.calls.some(([command, args]) =>
      command === "ssh_disconnect" && args.sessionId === rootSession)).toBe(false);
  });
});
