import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import { fromStore, writable } from "svelte/store";
import AppShell from "./AppShell.svelte";
import * as app from "../stores/app.svelte.ts";
import * as layout from "../stores/layout.svelte.ts";
import { setLocale, t } from "../i18n/index.svelte.ts";
import type { RuntimeCapabilities } from "../stores/runtime.svelte.ts";

const { invokeMock, listenMock, capabilitiesMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
  capabilitiesMock: vi.fn(() => ({ systemBack: false })),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ label: "settings-window" }) }));
vi.mock("../stores/runtime.svelte.ts", async (importOriginal) => ({
  ...await importOriginal<typeof import("../stores/runtime.svelte.ts")>(),
  capabilities: capabilitiesMock,
}));

const capability = fromStore(writable<RuntimeCapabilities>({
  localPty: false, serial: false, localDiscovery: false, sshAgent: false,
  defaultKeyFiles: false, cliInstall: false, multiWindow: false, windowControls: false,
  windowPin: false, fileMultiSelect: false, directoryTransfer: false, plugins: false,
  nativeClipboard: false, releaseUpdateCheck: false, systemBack: false,
}));
const eventName = "navigation:back:settings-window";
const callbacks = new Map<string, (event: { payload: null }) => void>();
const stops = new Map<string, ReturnType<typeof vi.fn>>();
const operations: string[] = [];
let component: ReturnType<typeof mount> | undefined;
let stopViewport: (() => void) | undefined;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

async function renderShell() {
  const target = document.createElement("div");
  document.body.append(target);
  component = mount(AppShell, { target });
  await settle();
  return target;
}

async function nativeBack() {
  const callback = callbacks.get(eventName);
  expect(callback).toBeDefined();
  callback!({ payload: null });
  await settle();
}

function nativeUpdates() {
  return invokeMock.mock.calls
    .filter(([command]) => command === "set_back_navigation_enabled")
    .map(([, args]) => args.enabled);
}

beforeEach(() => {
  callbacks.clear();
  stops.clear();
  operations.length = 0;
  capability.current = { ...capability.current, systemBack: false, nativeClipboard: false };
  capabilitiesMock.mockImplementation(() => capability.current);
  invokeMock.mockReset().mockImplementation(async (command: string, args?: { enabled?: boolean }) => {
    if (command === "set_back_navigation_enabled") operations.push(`set:${args?.enabled}`);
    return command === "get_setting" ? null : [];
  });
  listenMock.mockReset().mockImplementation(async (event: string, callback: (event: { payload: null }) => void) => {
    operations.push(`listen:${event}`);
    callbacks.set(event, callback);
    const stop = vi.fn(() => { operations.push(`unlisten:${event}`); callbacks.delete(event); });
    stops.set(event, stop);
    return stop;
  });
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
  await settle();
  if (stops.has(eventName)) await vi.waitFor(() => expect(stops.get(eventName)).toHaveBeenCalledOnce());
  stopViewport?.();
  stopViewport = undefined;
  app.closeSettings();
  document.body.innerHTML = "";
});

describe("AppShell system back integration", () => {
  it("keeps native interception disabled on hosts without the capability", async () => {
    app.openSettings();
    await renderShell();
    app.settingsNavigate("credential-edit");
    await settle();

    expect(listenMock).not.toHaveBeenCalledWith(eventName, expect.any(Function));
    expect(nativeUpdates()).toEqual([]);
  });

  it("subscribes when supported, handles each settings level, and disables at the workspace", async () => {
    app.openSettings();
    await renderShell();
    expect(nativeUpdates()).toEqual([]);

    capability.current = { ...capability.current, systemBack: true };
    await settle();
    await vi.waitFor(() => expect(nativeUpdates()).toEqual([true]));
    expect(listenMock).toHaveBeenCalledWith(eventName, expect.any(Function));
    expect(operations.indexOf(`listen:${eventName}`)).toBeLessThan(operations.indexOf("set:true"));

    // Refreshing an unrelated capability must retain the same subscription.
    capability.current = { ...capability.current, nativeClipboard: true };
    await settle();
    expect(listenMock.mock.calls.filter(([event]) => event === eventName)).toHaveLength(1);
    expect(stops.get(eventName)).not.toHaveBeenCalled();
    expect(nativeUpdates()).toEqual([true]);

    app.settingsNavigate("credential-edit");
    await settle();
    await nativeBack();
    expect(app.settingsPage()).toBe("credentials");
    await nativeBack();
    expect(app.settingsPage()).toBe("menu");
    expect(app.settingsActive()).toBe(true);
    expect(nativeUpdates()).toEqual([true]);
    await nativeBack();
    expect(app.settingsActive()).toBe(false);
    await vi.waitFor(() => expect(nativeUpdates()).toEqual([true, false]));
  });

  it("closes a real settings confirmation modal before navigating its page", async () => {
    capability.current = { ...capability.current, systemBack: true };
    app.navigate("shell-settings");
    const target = await renderShell();
    await vi.waitFor(() => expect(nativeUpdates()).toEqual([true]));
    target.querySelector<HTMLButtonElement>("#rca-select")!.click();
    await settle();
    const paste = [...target.querySelectorAll<HTMLButtonElement>(".rssh-select-option")]
      .find(button => button.textContent?.trim() === t("settings.shell.right_click_paste"));
    expect(paste).toBeDefined();
    paste!.click();
    await settle();
    expect(target.querySelector('[role="dialog"][aria-labelledby="rca-confirm-title"]')).not.toBeNull();

    await nativeBack();
    expect(target.querySelector('[role="dialog"][aria-labelledby="rca-confirm-title"]')).toBeNull();
    expect(app.settingsPage()).toBe("shell-settings");
    expect(nativeUpdates()).toEqual([true]);
    await nativeBack();
    expect(app.settingsPage()).toBe("menu");
    expect(app.settingsActive()).toBe(true);
  });

  it("disables interception before unsubscribing when the shell is unmounted", async () => {
    capability.current = { ...capability.current, systemBack: true };
    app.navigate("credential-edit");
    await renderShell();
    await vi.waitFor(() => expect(nativeUpdates()).toEqual([true]));
    const staleCallback = callbacks.get(eventName)!;
    operations.length = 0;

    await unmount(component!);
    component = undefined;
    await vi.waitFor(() => expect(stops.get(eventName)).toHaveBeenCalledOnce());
    expect(operations).toEqual(["set:false", `unlisten:${eventName}`]);
    staleCallback({ payload: null });
    expect(app.settingsPage()).toBe("credential-edit");
  });
});
