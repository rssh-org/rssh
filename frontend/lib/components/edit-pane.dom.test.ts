import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import { EditorView } from "codemirror";
import EditPane from "./EditPane.svelte";
import * as app from "../stores/app.svelte.ts";
import { setLocale } from "../i18n/index.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn(async () => null));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

let component: ReturnType<typeof mount> | undefined;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

async function render() {
  app.addTab({ id: "edit", type: "edit", label: "Broadcast" });
  component = mount(EditPane, { target: document.body, props: { tabId: "edit", active: true } });
  await settle();
}

function registerSession(tabId: string) {
  app.registerSession({ tabId, sessionId: `session:${tabId}`, type: "local" });
}

function card(label: string) {
  return [...document.querySelectorAll<HTMLElement>(".session-item")]
    .find(item => item.querySelector(".session-label")?.textContent === label)!;
}

function checkbox(label: string) {
  return card(label).querySelector<HTMLInputElement>('input[type="checkbox"]')!;
}

function broadcastButton() {
  return document.querySelector<HTMLButtonElement>(".broadcast-btn")!;
}

beforeEach(() => {
  invokeMock.mockClear();
  setLocale("en");
  // Canvas rendering is unrelated to target selection and unavailable in happy-dom.
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  for (const tab of app.tabs()) {
    app.unregisterSession(tab.id);
    app.unregisterTerminalControls(tab.id);
  }
  for (const tab of app.workspaceTabs()) app.closeTab(tab.id);
  app.setActiveTab("home");
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("broadcast editor target sessions", () => {
  it("uses the checkbox to select targets without navigating or changing other targets", async () => {
    app.addTab({ id: "first", type: "local", label: "First shell" });
    app.addTab({ id: "second", type: "local", label: "Second shell" });
    registerSession("first");
    registerSession("second");
    await render();

    expect(broadcastButton().disabled).toBe(true);
    card("First shell").querySelector<HTMLLabelElement>(".session-select")!.click();
    await settle();

    expect(checkbox("First shell").checked).toBe(true);
    expect(checkbox("Second shell").checked).toBe(false);
    expect(app.activeWorkspaceId()).toBe("edit");
    expect(broadcastButton().textContent).toContain("(1)");

    checkbox("First shell").click();
    await settle();
    expect(checkbox("First shell").checked).toBe(false);
    expect(broadcastButton().disabled).toBe(true);
    expect(app.activeWorkspaceId()).toBe("edit");
  });

  it.each(["standalone", "root", "nested"] as const)(
    "opens the exact %s terminal from a card without changing broadcast selection",
    async (targetKind) => {
      app.addTab({ id: "root", type: "local", label: "Root shell" });
      registerSession("root");
      if (targetKind !== "standalone") {
        app.addPane("root", "right", { id: "sibling", type: "local", label: "Sibling shell" });
        app.addPane("root", "bottom", { id: "nested", type: "local", label: "Nested shell" });
        registerSession("sibling");
        registerSession("nested");
        app.setActivePane("sibling");
      }
      const targetId = targetKind === "nested" ? "nested" : "root";
      const targetLabel = targetKind === "nested" ? "Nested shell" : "Root shell";
      const layout = app.layoutForWorkspace("root");
      await render();
      checkbox("Root shell").click();
      await settle();
      const selectedBefore = [...document.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')]
        .map(input => input.checked);

      const openButton = card(targetLabel).querySelector<HTMLButtonElement>(".session-open")!;
      openButton.dispatchEvent(new MouseEvent("mouseenter"));
      await settle();
      expect(document.querySelector(".preview-popover")).not.toBeNull();
      openButton.click();
      await settle();

      expect(app.activeWorkspaceId()).toBe("root");
      expect(app.activePaneId()).toBe(targetId);
      expect(app.activeTabId()).toBe(targetId);
      expect(document.querySelector(".preview-popover")).toBeNull();
      expect(app.layoutForWorkspace("root")).toEqual(layout);
      expect([...document.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')].map(input => input.checked))
        .toEqual(selectedBefore);
      expect(app.workspaceTabs().map(tab => tab.id)).toEqual(["root", "edit"]);

      app.setActiveWorkspace("edit");
      app.setActiveWorkspace("root");
      expect(app.activePaneId()).toBe(targetId);
    },
  );

  it("broadcasts only to checked targets and removes disconnected targets from selection", async () => {
    const firstSend = vi.fn();
    const secondSend = vi.fn();
    for (const [id, sendText] of [["first", firstSend], ["second", secondSend]] as const) {
      app.addTab({ id, type: "local", label: id });
      registerSession(id);
      app.registerTerminalControls(id, {
        getSelection: () => "", paste: vi.fn(), writeControl: vi.fn(), sendArrow: vi.fn(),
        sendText, focus: vi.fn(),
      });
    }
    await render();
    const editor = EditorView.findFromDOM(document.querySelector(".cm-editor")!)!;
    editor.dispatch({ changes: { from: 0, insert: "echo hello" } });
    checkbox("second").click();
    await settle();
    broadcastButton().click();
    await settle();

    expect(firstSend).not.toHaveBeenCalled();
    expect(secondSend).toHaveBeenCalledExactlyOnceWith("echo hello\n");
    expect(app.activeWorkspaceId()).toBe("edit");

    app.unregisterSession("second");
    await settle();
    expect(document.querySelectorAll('input[type="checkbox"]')).toHaveLength(1);
    expect(broadcastButton().disabled).toBe(true);
  });
});
