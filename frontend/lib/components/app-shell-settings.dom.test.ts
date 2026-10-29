import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import AppShell from "./AppShell.svelte";
import { lifecycle } from "./test-fixtures/TerminalWorkspace.svelte";
import * as app from "../stores/app.svelte.ts";
import * as layout from "../stores/layout.svelte.ts";
import * as back from "../stores/back.svelte.ts";
import * as keymap from "../stores/keymap.svelte.ts";
import { setLocale, t } from "../i18n/index.svelte.ts";

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
  invokeMock.mockReset().mockImplementation(async (command: string) => command === "get_setting" ? null : []);
  lifecycle.mounted = 0;
  lifecycle.destroyed = 0;
  app.closeSettings();
  app.closeDownloads();
  app.closeSnippetPicker();
  app.settingsNavigate("menu");
  setLocale("en");
  window.innerWidth = 400;
  stopViewport = layout.observeViewport();
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  stopViewport?.();
  stopViewport = undefined;
  app.closeSettings();
  for (const tab of app.workspaceTabs()) if (tab.id !== "home") app.closeTab(tab.id);
  document.body.innerHTML = "";
});

describe("AppShell compact settings", () => {
  it("ignores an open dialog in a hidden terminal workspace and preserves it when returning", async () => {
    app.addTab({ id: "ssh:hidden-dialog", type: "ssh", label: "Production" });
    const target = document.createElement("div");
    document.body.append(target);
    component = mount(AppShell, { target });
    await settle();
    target.querySelector<HTMLButtonElement>('[data-testid="terminal-workspace"] button')!.click();
    await settle();
    const dialog = target.querySelector('[role="dialog"]');
    expect(dialog).not.toBeNull();

    app.setActiveWorkspace("home");
    app.openSettings();
    await settle();
    target.querySelector<HTMLButtonElement>('header button[aria-label="Back"]')!.click();
    await settle();
    expect(app.settingsActive()).toBe(false);
    expect(back.canHandleBack()).toBe(false);
    expect(target.querySelector('[role="dialog"]')).toBe(dialog);

    app.setActiveWorkspace("ssh:hidden-dialog");
    await settle();
    expect(back.canHandleBack()).toBe(true);
    expect(back.requestBack()).toBe(true);
    await settle();
    expect(target.querySelector('[role="dialog"]')).toBeNull();
    expect(back.canHandleBack()).toBe(false);
  });

  it.each([
    ["left", ".sidebar-rail"],
    ["top", ".stripbar"],
  ] as const)("replaces the %s navigation and returns to the same terminal workspace", async (position, navigationSelector) => {
    app.setSidebarPosition(position);
    app.addTab({ id: "ssh:settings-return", type: "ssh", label: "Production" });
    const target = document.createElement("div");
    document.body.append(target);
    component = mount(AppShell, { target });
    await settle();

    const terminal = target.querySelector<HTMLElement>('[data-testid="terminal-workspace"]')!;
    const terminalPane = terminal.closest<HTMLElement>(".terminal-layout-pane")!;
    const terminalInput = terminal.querySelector<HTMLTextAreaElement>("textarea")!;
    terminalInput.value = "unfinished terminal input";
    expect(lifecycle.mounted).toBe(1);
    expect(terminalPane.classList.contains("visible")).toBe(true);
    expect(terminalPane.inert).toBe(false);
    expect(target.querySelector(navigationSelector)).not.toBeNull();

    target.querySelector<HTMLButtonElement>(`button[title="${t("tab.settings")}"]`)!.click();
    await settle();
    expect(target.querySelector(".shell.settings-fullscreen")).not.toBeNull();
    expect(target.querySelector(".sidebar-rail, .stripbar")).toBeNull();
    expect(terminalPane.classList.contains("visible")).toBe(false);
    expect(terminalPane.inert).toBe(true);
    expect(target.querySelector('[data-testid="terminal-workspace"]')).toBe(terminal);

    // Global tab shortcuts must not activate the workspace hidden behind settings.
    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "1", code: "Digit1", altKey: !keymap.isMac, metaKey: keymap.isMac, bubbles: true,
    }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", ctrlKey: true, bubbles: true }));
    window.dispatchEvent(new KeyboardEvent("keyup", { key: "Control", bubbles: true }));
    await settle();
    expect(app.settingsActive()).toBe(true);
    expect(app.activeWorkspaceId()).toBe("ssh:settings-return");

    app.settingsNavigate("credential-edit");
    await settle();
    const name = target.querySelector<HTMLInputElement>('input[placeholder="prod-key"]')!;
    name.value = "Unsaved credential";
    name.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    await resize(900);
    expect(target.querySelector(".shell.settings-fullscreen")).toBeNull();
    expect(target.querySelector(".sidebar-rail, .stripbar")).not.toBeNull();
    expect(target.querySelector('input[placeholder="prod-key"]')).toBe(name);
    expect(name.value).toBe("Unsaved credential");
    expect(target.querySelector('[data-testid="terminal-workspace"]')).toBe(terminal);
    expect(terminalPane.inert).toBe(true);

    await resize(400);
    expect(target.querySelector(".sidebar-rail, .stripbar")).toBeNull();
    expect(target.querySelector('input[placeholder="prod-key"]')).toBe(name);
    expect(name.value).toBe("Unsaved credential");

    for (const expectedPage of ["credentials", "menu"] as const) {
      target.querySelector<HTMLButtonElement>('header button[aria-label="Back"]')!.click();
      await settle();
      expect(app.settingsPage()).toBe(expectedPage);
    }
    target.querySelector<HTMLButtonElement>('header button[aria-label="Back"]')!.click();
    await settle();

    expect(app.settingsActive()).toBe(false);
    expect(app.activeTabId()).toBe("ssh:settings-return");
    expect(app.activeWorkspaceId()).toBe("ssh:settings-return");
    expect(target.querySelector(navigationSelector)).not.toBeNull();
    expect(target.querySelector('[data-testid="terminal-workspace"]')).toBe(terminal);
    expect(terminalPane.classList.contains("visible")).toBe(true);
    expect(terminalPane.inert).toBe(false);
    expect(terminalInput.value).toBe("unfinished terminal input");
    expect(lifecycle).toEqual({ mounted: 1, destroyed: 0 });
  });
});
