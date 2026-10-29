import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRawSnippet, flushSync, mount, tick, unmount, type Component } from "svelte";
import { fromStore, writable } from "svelte/store";
import Modal from "./Modal.svelte";
import Select from "./Select.svelte";
import SearchSelect from "./SearchSelect.svelte";
import DownloadsScreen from "./DownloadsScreen.svelte";
import TabContextMenu from "./TabContextMenu.svelte";
import SnippetPicker from "./SnippetPicker.svelte";
import BackModal from "./test-fixtures/BackModal.svelte";
import BackPanel from "./test-fixtures/BackPanel.svelte";
import * as app from "../stores/app.svelte.ts";
import { canHandleBack, registerBackHandler, requestBack } from "../stores/back.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

const mounted: ReturnType<typeof mount>[] = [];
let removePage: (() => void) | undefined;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

async function render(component: Component<any>, props: Record<string, unknown> = {}, target?: HTMLElement) {
  const element = target ?? document.createElement("div");
  if (!target) document.body.append(element);
  const instance = mount(component, { target: element, props });
  mounted.push(instance);
  await settle();
  return { element, instance };
}

async function destroy(instance: ReturnType<typeof mount>) {
  await unmount(instance);
  mounted.splice(mounted.indexOf(instance), 1);
  await settle();
}

beforeEach(() => {
  invokeMock.mockReset().mockResolvedValue([]);
  app.closeDownloads();
  app.closeSnippetPicker();
  app.navigate("credentials");
  removePage = registerBackHandler({
    handle: app.settingsBack,
    enabled: app.settingsCanGoBack,
    priority: 0,
  });
});

afterEach(async () => {
  for (const instance of mounted.splice(0).reverse()) await unmount(instance);
  removePage?.();
  removePage = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("system back dismisses transient UI", () => {
  it.each([
    ["hidden panel", "hidden", Select],
    ["inactive pane", "visible", SearchSelect],
  ] as const)("ignores saved modal and dropdown state in a %s", async (_, flag, dropdown) => {
    const state = fromStore(writable({ hidden: false, visible: true }));
    const closeModal = vi.fn();
    const enabledChanged = vi.fn();
    const panel = await render(BackPanel, {
      get hidden() { return state.current.hidden; },
      get visible() { return state.current.visible; },
      dropdown,
      onClose: closeModal,
      onEnabledChange: enabledChanged,
    });
    const trigger = panel.element.querySelector<HTMLButtonElement>('[aria-label="Choice"]')!;
    trigger.click();
    await settle();

    state.current = { ...state.current, [flag]: flag === "hidden" };
    await settle();
    expect(requestBack()).toBe(true);
    expect(app.settingsPage()).toBe("menu");
    expect(closeModal).not.toHaveBeenCalled();
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    expect(requestBack()).toBe(true);
    expect(app.settingsActive()).toBe(false);
    expect(canHandleBack()).toBe(false);
    await settle();
    expect(enabledChanged).toHaveBeenLastCalledWith(false);

    state.current = { hidden: false, visible: true };
    await settle();
    expect(canHandleBack()).toBe(true);
    expect(enabledChanged).toHaveBeenLastCalledWith(true);
    expect(panel.element.querySelector('[aria-label="Choice"]')).toBe(trigger);
    expect(requestBack()).toBe(true);
    await settle();
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    expect(closeModal).not.toHaveBeenCalled();
    expect(requestBack()).toBe(true);
    await settle();
    expect(closeModal).toHaveBeenCalledOnce();
    expect(canHandleBack()).toBe(false);
  });

  it.each([
    ["Select", Select],
    ["SearchSelect", SearchSelect],
  ] as const)("closes %s before its modal and keeps settings underneath unchanged", async (_, dropdown) => {
    const closeModal = vi.fn();
    const modal = await render(BackModal, { dropdown, onClose: closeModal });
    const trigger = modal.element.querySelector<HTMLButtonElement>('[aria-label="Choice"]')!;
    trigger.click();
    await settle();
    expect(trigger.getAttribute("aria-expanded")).toBe("true");

    expect(requestBack()).toBe(true);
    await settle();
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    expect(closeModal).not.toHaveBeenCalled();
    expect(app.settingsPage()).toBe("credentials");

    expect(requestBack()).toBe(true);
    await settle();
    expect(closeModal).toHaveBeenCalledOnce();
    expect(app.settingsPage()).toBe("credentials");
    expect(modal.element.querySelector('[role="dialog"]')).toBeNull();
    await destroy(modal.instance);

    expect(requestBack()).toBe(true);
    expect(app.settingsPage()).toBe("menu");
    removePage?.();
    removePage = undefined;
    expect(canHandleBack()).toBe(false);
  });

  it("dismisses a new modal above a dropdown left open on the underlying page", async () => {
    const select = await render(Select, {
      value: "a", options: [{ value: "a", label: "A" }], ariaLabel: "Page choice",
    });
    const trigger = select.element.querySelector<HTMLButtonElement>('[aria-label="Page choice"]')!;
    trigger.click();
    await settle();
    const closeModal = vi.fn();
    const modal = await render(Modal, {
      onClose: closeModal,
      children: createRawSnippet(() => ({ render: () => '<div>Confirm</div>' })),
    });

    expect(requestBack()).toBe(true);
    await settle();
    expect(closeModal).toHaveBeenCalledOnce();
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    expect(app.settingsPage()).toBe("credentials");

    await destroy(modal.instance);
    expect(requestBack()).toBe(true);
    await settle();
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    expect(app.settingsPage()).toBe("credentials");
  });

  it("closes the transfer popover without leaving the settings page", async () => {
    app.openDownloads();
    const popover = await render(DownloadsScreen);

    expect(requestBack()).toBe(true);
    expect(app.downloadsActive()).toBe(false);
    expect(app.settingsPage()).toBe("credentials");
    await destroy(popover.instance);
    expect(requestBack()).toBe(true);
    expect(app.settingsPage()).toBe("menu");
  });

  it("dismisses a context menu without navigating its page", async () => {
    const close = vi.fn();
    await render(TabContextMenu, { x: 0, y: 0, sections: [], onClose: close });

    expect(requestBack()).toBe(true);
    expect(close).toHaveBeenCalledOnce();
    expect(app.settingsPage()).toBe("credentials");
  });

  it("closes the snippet picker without navigating its page", async () => {
    app.openSnippetPicker();
    await render(SnippetPicker);

    expect(requestBack()).toBe(true);
    expect(app.snippetPickerOpen()).toBe(false);
    expect(app.settingsPage()).toBe("credentials");
  });
});
