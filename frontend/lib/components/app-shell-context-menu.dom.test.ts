import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import AppShell from "./AppShell.svelte";
import * as app from "../stores/app.svelte.ts";
import * as layout from "../stores/layout.svelte.ts";
import { installInputTracking } from "../input.ts";
import { t } from "../i18n/index.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("./TerminalSplitLayout.svelte", async () => import("./test-fixtures/TerminalWorkspace.svelte"));

const tabId = "ssh:tab-context-menu";
let component: ReturnType<typeof mount> | undefined;
let stopViewport: (() => void) | undefined;
let stopInputTracking: (() => void) | undefined;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

beforeEach(() => {
  invokeMock.mockReset().mockImplementation(async (command: string) => {
    if (command === "get_setting" || command === "ai_settings_get") return null;
    return [];
  });
  app.closeSettings();
  app.closeDownloads();
  app.closeSnippetPicker();
  stopInputTracking = installInputTracking();
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  stopInputTracking?.();
  stopViewport?.();
  app.closeTab(tabId);
  document.body.replaceChildren();
});

describe("Tab context menus remain available independently of terminal menus", () => {
  it.each([
    { width: 400, position: "top" as const },
    { width: 1200, position: "left" as const },
  ])("keeps long-press and mouse menus in the $position navigation at $width px", async ({ width, position }) => {
    window.innerWidth = width;
    stopViewport = layout.observeViewport();
    app.setSidebarPosition(position);
    app.addTab({ id: tabId, type: "ssh", label: "Menu test session" });
    const target = document.createElement("div");
    document.body.append(target);
    component = mount(AppShell, { target });
    await settle();
    const tab = target.querySelector<HTMLButtonElement>('button.sb-item[title="Menu test session"]')!;
    expect(tab).not.toBeNull();

    for (const pointerType of ["touch", "pen", "mouse"]) {
      // Test both current PointerEvent context menus and older WebViews that
      // send MouseEvent after a touch/pen pointerdown.
      for (const legacy of [false, true]) {
        tab.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerType }));
        const init = { bubbles: true, cancelable: true, button: 2 };
        const event = legacy
          ? new MouseEvent("contextmenu", init)
          : new PointerEvent("contextmenu", { ...init, pointerType });
        tab.dispatchEvent(event);
        await settle();

        expect(event.defaultPrevented).toBe(true);
        const labels = [...target.querySelectorAll(".ctx-menu .ctx-label")].map((item) => item.textContent);
        expect(labels).toContain(t("tab.context.close"));
        expect(labels).toContain(t("tab.context.close_others"));
        expect(labels).toContain(t("tab.context.clone"));

        target.querySelector<HTMLElement>(".ctx-backdrop")!.click();
        await settle();
        expect(target.querySelector(".ctx-menu")).toBeNull();
      }
    }
  });
});
