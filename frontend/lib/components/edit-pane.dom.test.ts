import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import { fromStore, writable } from "svelte/store";
import { EditorView } from "codemirror";
import EditPane from "./EditPane.svelte";
import * as app from "../stores/app.svelte.ts";
import { setLocale } from "../i18n/index.svelte.ts";
import * as theme from "../themes/store.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn(async () => null));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

let component: ReturnType<typeof mount> | undefined;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

async function render(containerWidth = 1000) {
  // happy-dom has no layout; supply the width observed by bind:clientWidth.
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(function (this: HTMLElement) {
    return this.classList.contains("edit-pane") ? containerWidth : 0;
  });
  const active = writable(true);
  const state = fromStore(active);
  app.addTab({ id: "edit", type: "edit", label: "Broadcast" });
  component = mount(EditPane, {
    target: document.body,
    props: { tabId: "edit", get active() { return state.current; } },
  });
  await settle();
  return active;
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

function registerPreview(tabId: string, readViewportText: (recentRows?: number) => string[] | null) {
  app.addTab({ id: tabId, type: "local", label: tabId });
  registerSession(tabId);
  app.registerTerminalControls(tabId, {
    getSelection: () => "", paste: vi.fn(), writeControl: vi.fn(), sendArrow: vi.fn(),
    sendText: vi.fn(), focus: vi.fn(), readViewportText,
  });
}

function preview(tabId: string) {
  return card(tabId).querySelector<HTMLPreElement>(".session-preview")!;
}

function resizeFixture(containerWidth = 1000, renderedWidth = 320) {
  const separator = document.querySelector<HTMLElement>(".session-resize")!;
  expect(separator).not.toBeNull();
  vi.spyOn(document.querySelector<HTMLElement>(".edit-pane")!, "getBoundingClientRect")
    .mockReturnValue(new DOMRect(0, 0, containerWidth, 600));
  vi.spyOn(document.querySelector<HTMLElement>(".session-panel")!, "getBoundingClientRect")
    .mockReturnValue(new DOMRect(containerWidth - renderedWidth, 0, renderedWidth, 600));
  let captured: number | undefined;
  Object.assign(separator, {
    setPointerCapture: vi.fn((pointerId: number) => { captured = pointerId; }),
    hasPointerCapture: vi.fn((pointerId: number) => captured === pointerId),
    releasePointerCapture: vi.fn(() => { captured = undefined; }),
  });
  return separator;
}

async function pointer(target: EventTarget, type: string, x: number, pointerType = "mouse", pointerId = 1) {
  target.dispatchEvent(new PointerEvent(type, {
    pointerId, pointerType, clientX: x, button: 0,
    buttons: type === "pointerup" ? 0 : 1, bubbles: true, cancelable: true,
  }));
  await settle();
}

function panelWidth(separator: HTMLElement) {
  return Number(separator.getAttribute("aria-valuenow"));
}

beforeEach(() => {
  invokeMock.mockClear();
  setLocale("en");
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
  await theme.setTermFontSize(theme.termFontSizeBounds.def);
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("broadcast editor target sessions", () => {
  it("refreshes each target's readable recent rows at the terminal's actual font size", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    let firstLines = ["first: line 1", "", "first: line 3", "中文 e\u0301", "first $ "];
    const firstRead = vi.fn(() => firstLines);
    const secondRead = vi.fn(() => ["second: ready", "second $ "]);
    registerPreview("first", firstRead);
    registerPreview("second", secondRead);
    await theme.setTermFontSize(19);
    await render();

    expect(firstRead).toHaveBeenCalledWith(5);
    expect(secondRead).toHaveBeenCalledWith(5);
    expect(preview("first").textContent).toBe(firstLines.join("\n"));
    expect(preview("second").textContent).toBe("second: ready\nsecond $ ");
    expect(preview("first").style.fontSize).toBe("19px");
    expect(preview("second").style.fontSize).toBe("19px");

    firstLines = ["a long line " + "x".repeat(200), "first: finished"];
    await vi.advanceTimersByTimeAsync(300);
    await settle();
    expect(preview("first").textContent).toBe(firstLines.join("\n"));
    expect(preview("second").textContent).toBe("second: ready\nsecond $ ");

    await theme.setTermFontSize(24);
    await settle();
    expect(preview("first").style.fontSize).toBe("24px");
    expect(preview("second").style.fontSize).toBe("24px");

    await unmount(component!);
    component = undefined;
    firstRead.mockClear();
    secondRead.mockClear();
    await vi.advanceTimersByTimeAsync(900);
    expect(firstRead).not.toHaveBeenCalled();
    expect(secondRead).not.toHaveBeenCalled();
  });

  it("keeps the enlarged hover preview reading the full viewport at the terminal font size", async () => {
    const rows = ["viewport top", "line 2", "line 3", "line 4", "line 5", "recent bottom"];
    const read = vi.fn((recentRows?: number) => recentRows ? rows.slice(-recentRows) : rows);
    registerPreview("first", read);
    await theme.setTermFontSize(18);
    await render();
    const openButton = card("first").querySelector<HTMLButtonElement>(".session-open")!;
    openButton.dispatchEvent(new MouseEvent("mouseenter"));
    await settle();

    expect(preview("first").textContent).toBe(rows.slice(-5).join("\n"));
    const enlarged = document.querySelector<HTMLPreElement>(".preview-popover pre")!;
    expect(enlarged.textContent).toBe(rows.join("\n"));
    expect(enlarged.style.fontSize).toBe("18px");
    openButton.dispatchEvent(new MouseEvent("mouseleave"));
    await settle();
    expect(document.querySelector(".preview-popover")).toBeNull();
  });

  it("keeps the hover preview's last output visible after refreshes and font changes", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    let rows = [...Array.from({ length: 30 }, (_, index) => `output ${index + 1}`), "", "finished", "", ""];
    let contentHeight = 900;
    vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(function (this: HTMLElement) {
      return this.classList.contains("preview-popover") ? contentHeight : 0;
    });
    registerPreview("first", (recentRows) => recentRows ? rows.slice(-recentRows) : rows);
    await render();
    card("first").querySelector<HTMLButtonElement>(".session-open")!
      .dispatchEvent(new MouseEvent("mouseenter"));
    await settle();

    const popover = document.querySelector<HTMLElement>(".preview-popover")!;
    const text = popover.querySelector("pre")!;
    // happy-dom does not perform scrolling layout; this checks the requested
    // scroll position. Browser validation covers the actual visible last row.
    expect(popover.scrollTop).toBe(900);
    expect(text.textContent).toBe(rows.join("\n").trimEnd());
    expect(text.textContent).toContain("output 30\n\nfinished");

    rows = [...rows, "new output", "new prompt $", "", ""];
    contentHeight = 1200;
    await vi.advanceTimersByTimeAsync(300);
    await settle();
    expect(popover.scrollTop).toBe(1200);
    expect(text.textContent).toBe(rows.join("\n").trimEnd());
    expect(text.textContent).toMatch(/new output\nnew prompt \$$/);

    contentHeight = 1500;
    await theme.setTermFontSize(24);
    await settle();
    expect(text.style.fontSize).toBe("24px");
    expect(popover.scrollTop).toBe(1500);
  });

  it("drops unused viewport rows after short output without removing interior blank lines", async () => {
    const rows = ["  server ready", "", "shell $ ", ...Array<string>(24).fill("")];
    registerPreview("first", () => rows);
    await render();
    card("first").querySelector<HTMLButtonElement>(".session-open")!
      .dispatchEvent(new MouseEvent("mouseenter"));
    await settle();

    const popover = document.querySelector<HTMLElement>(".preview-popover")!;
    expect(popover.querySelector("pre")!.textContent).toBe("  server ready\n\nshell $");
    expect(popover.scrollTop).toBe(0);
  });

  it("stops reading hidden or removed targets and immediately refreshes on activation", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const read = vi.fn<(...args: [number?]) => string[] | null>(() => null);
    registerPreview("first", read);
    const active = await render();
    expect(preview("first").textContent).toBe("");

    active.set(false);
    await settle();
    read.mockClear().mockReturnValue(["updated while hidden"]);
    await vi.advanceTimersByTimeAsync(900);
    await settle();
    expect(read).not.toHaveBeenCalled();

    active.set(true);
    await settle();
    expect(preview("first").textContent).toBe("updated while hidden");
    expect(read).toHaveBeenCalledExactlyOnceWith(5);
    app.unregisterSession("first");
    await settle();
    read.mockClear();
    await vi.advanceTimersByTimeAsync(900);
    expect(read).not.toHaveBeenCalled();
    expect(document.querySelector(".session-preview")).toBeNull();
  });

  it.each(["mouse", "touch"])("resizes from the rendered width using %s and preserves editor space", async (pointerType) => {
    await render();
    const separator = resizeFixture(1000, 300);
    expect(separator.getAttribute("aria-valuemin")).toBe("160");
    expect(separator.getAttribute("aria-valuemax")).toBe("700");
    await pointer(separator, "pointerdown", 700, pointerType);
    await pointer(window, "pointermove", 600, pointerType);
    expect(panelWidth(separator)).toBe(400);

    await pointer(window, "pointermove", 500, pointerType, 2);
    expect(panelWidth(separator)).toBe(400);
    await pointer(window, "pointermove", -1000, pointerType);
    expect(panelWidth(separator)).toBeLessThanOrEqual(700);
    expect(panelWidth(separator)).toBeGreaterThan(400);
    await pointer(window, "pointermove", 2000, pointerType);
    expect(panelWidth(separator)).toBe(160);
    await pointer(window, "pointerup", 2000, pointerType);
    await pointer(window, "pointermove", 600, pointerType);
    expect(panelWidth(separator)).toBe(160);
    expect(separator.releasePointerCapture).toHaveBeenCalledWith(1);
  });

  it("keeps the editor's share of a narrow container during touch resizing", async () => {
    await render(300);
    const separator = resizeFixture(300, 210);
    expect(separator.getAttribute("aria-valuemax")).toBe("210");
    expect(panelWidth(separator)).toBe(210);
    await pointer(separator, "pointerdown", 90, "touch");
    await pointer(window, "pointermove", 0, "touch");
    expect(panelWidth(separator)).toBe(210);
    await pointer(window, "pointerup", 0, "touch");
  });

  it.each(["pointercancel", "blur", "hidden", "unmount"])("ends an in-progress resize on %s", async (end) => {
    const active = await render();
    const separator = resizeFixture();
    await pointer(separator, "pointerdown", 680, "touch");
    await pointer(window, "pointermove", 580, "touch");
    expect(panelWidth(separator)).toBe(420);
    if (end === "pointercancel") await pointer(window, "pointercancel", 580, "touch");
    if (end === "blur") window.dispatchEvent(new Event("blur"));
    if (end === "hidden") active.set(false);
    if (end === "unmount") {
      await unmount(component!);
      component = undefined;
    }
    await settle();
    await pointer(window, "pointermove", 480, "touch");
    expect(panelWidth(separator)).toBe(420);
    expect(separator.releasePointerCapture).toHaveBeenCalledWith(1);
  });

  it("allows keyboard resizing and double-click reset", async () => {
    await render();
    const separator = resizeFixture();
    separator.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true, cancelable: true }));
    await settle();
    expect(panelWidth(separator)).toBe(344);
    vi.spyOn(document.querySelector<HTMLElement>(".session-panel")!, "getBoundingClientRect")
      .mockReturnValue(new DOMRect(656, 0, 344, 600));
    separator.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    await settle();
    expect(panelWidth(separator)).toBe(320);
    await pointer(separator, "pointerdown", 656);
    await pointer(window, "pointermove", 556);
    await pointer(window, "pointerup", 556);
    expect(panelWidth(separator)).toBe(444);
    separator.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await settle();
    expect(panelWidth(separator)).toBe(320);
  });

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
