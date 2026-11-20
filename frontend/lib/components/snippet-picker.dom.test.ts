import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import SnippetPicker from "./SnippetPicker.svelte";
import * as app from "../stores/app.svelte.ts";
import { requestBack } from "../stores/back.svelte.ts";
import { toast } from "../stores/toast.svelte.ts";
import { setLocale } from "../i18n/index.svelte.ts";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const repeated: app.Snippet = {
  name: "Service", command: "logs --lines=500 {yyy} /srv/nginx\nstatus nginx",
  parameters: [{ name: "nginx", defaultValue: "redis" }, { name: "500", defaultValue: null }],
};
const literal: app.Snippet = { name: "Service", command: "echo {yyy} ${HOME} '{{.Names}}'" };
let picker: ReturnType<typeof mount>;
let host: HTMLElement;

function controls() {
  return { sendText: vi.fn(), getSelection: vi.fn(), paste: vi.fn(), writeControl: vi.fn(), sendArrow: vi.fn(), focus: vi.fn() };
}
const left = controls();
const right = controls();

async function render(snippets = [literal, repeated]) {
  invoke.mockImplementation(async (command: string) => command === "load_snippets" ? snippets : null);
  app.openSnippetPicker();
  picker = mount(SnippetPicker, { target: host });
  await vi.waitFor(() => {
    flushSync();
    expect(host.querySelectorAll(".picker-item")).toHaveLength(snippets.length);
  });
}
function search() { return host.querySelector<HTMLInputElement>(".search")!; }
function inputs() { return [...host.querySelectorAll<HTMLTextAreaElement>(".fields textarea")]; }
function sendButton() { return host.querySelector<HTMLButtonElement>("footer button")!; }
function key(element: HTMLElement, key: string, options: KeyboardEventInit = {}) {
  element.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...options }));
  flushSync();
}
function edit(input: HTMLInputElement | HTMLTextAreaElement, text: string) {
  input.value = text;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
}

beforeEach(() => {
  setLocale("en");
  vi.clearAllMocks();
  host = document.createElement("div");
  document.body.append(host);
  app.addTab({ id: "left", type: "local", label: "Left" });
  app.addPane("left", "right", { id: "right", type: "local", label: "Right" });
  app.registerTerminalControls("left", left);
  app.registerTerminalControls("right", right);
  app.setTerminalConnectionStatus("left", "connected");
  app.setTerminalConnectionStatus("right", "connected");
  app.setActivePane("left");
});

afterEach(async () => {
  await unmount(picker);
  host.remove();
  app.closeSnippetPicker();
  app.closeTab("left");
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("sends an ordinary snippet literally with one click, even with duplicate names", async () => {
  await render();
  const rows = host.querySelectorAll<HTMLButtonElement>(".picker-item");
  expect(rows).toHaveLength(2);
  rows[0].click();
  expect(left.sendText).toHaveBeenCalledExactlyOnceWith(literal.command);
  expect(app.snippetPickerOpen()).toBe(false);
});

it("asks once for xxx and replaces both occurrences in echo yyy xxx zzz xxx", async () => {
  await render([{ name: "test", command: "echo yyy xxx zzz xxx", parameters: [{ name: "xxx", defaultValue: null }] }]);
  key(search(), "Enter");
  expect(inputs()).toHaveLength(1);
  expect(inputs()[0].value).toBe("");
  expect(sendButton().disabled).toBe(true);
  edit(inputs()[0], "abc");
  expect(host.querySelector("pre")?.textContent).toBe("echo yyy abc zzz abc");
  sendButton().click();
  expect(left.sendText).toHaveBeenCalledExactlyOnceWith("echo yyy abc zzz abc");
});

it("distinguishes whitespace labels and accepts multiline matches, defaults and typed values", async () => {
  await render([{
    name: "Multiline", command: "first\nsecond  first\nsecond",
    parameters: [{ name: "first\nsecond", defaultValue: "a\nb" }, { name: "  ", defaultValue: "|" }],
  }]);
  key(search(), "Enter");
  const [multiline] = inputs();
  expect(Array.from(host.querySelectorAll(".fields label > span"), label => label.textContent))
    .toEqual(['"first\\nsecond"', '"  "']);
  expect(multiline.value).toBe("a\nb");
  key(multiline, "Enter", { shiftKey: true });
  expect(left.sendText).not.toHaveBeenCalled();
  edit(multiline, "one\ntwo");
  key(multiline, "Enter");
  expect(left.sendText).toHaveBeenCalledExactlyOnceWith("one\ntwo|one\ntwo");
});

it("prompts in declared order and sends literal replacements to the opening pane", async ({ onTestFinished }) => {
  await render();
  key(search(), "ArrowDown");
  key(search(), "Enter");
  const [unit, lines] = inputs();
  expect(inputs().map(input => input.closest("label")!.textContent?.trim())).toEqual(["nginx", "500"]);
  expect(document.activeElement).toBe(unit);
  expect(inputs().map(input => input.value)).toEqual(["redis", ""]);
  expect(unit.value.slice(unit.selectionStart!, unit.selectionEnd!)).toBe("redis");
  expect(sendButton().disabled).toBe(true);
  expect(host.querySelectorAll("pre .active")).toHaveLength(2);
  edit(unit, "$&500");
  expect(host.querySelector("pre")?.textContent).toBe("logs --lines=500 {yyy} /srv/$&500\nstatus $&500");
  key(unit, "Enter");
  expect(document.activeElement).toBe(lines);
  edit(lines, "20");
  key(lines, "Enter", { isComposing: true });
  key(lines, "Enter", { repeat: true });
  expect(left.sendText).not.toHaveBeenCalled();
  expect(sendButton().disabled).toBe(false);

  app.setActivePane("right");
  flushSync();
  expect(host.querySelector(".target")?.textContent).toContain("Left");
  key(lines, "Enter");
  expect(left.sendText).toHaveBeenCalledExactlyOnceWith("logs --lines=20 {yyy} /srv/$&500\nstatus $&500");
  expect(right.sendText).not.toHaveBeenCalled();
  expect(app.snippetPickerOpen()).toBe(false);
  expect(app.activePaneId()).toBe("left");
  onTestFinished(() => {
    expect(left.focus).toHaveBeenCalledOnce();
    expect(right.focus).not.toHaveBeenCalled();
  });
});

it("returns to the originating split pane after switching workspaces", async ({ onTestFinished }) => {
  app.setActivePane("right");
  await render([literal]);
  app.setActivePane("left");
  app.addTab({ id: "other", type: "local", label: "Other workspace" });
  onTestFinished(() => app.closeTab("other"));
  expect(app.activeWorkspaceId()).toBe("other");

  key(search(), "Enter");
  expect(right.sendText).toHaveBeenCalledExactlyOnceWith(literal.command);
  expect(app.activeWorkspaceId()).toBe("left");
  expect(app.activePaneId()).toBe("right");
  onTestFinished(() => {
    expect(right.focus).toHaveBeenCalledOnce();
    expect(left.focus).not.toHaveBeenCalled();
  });
});

it("returns to the filtered row on system Back or Escape and discards unsent values", async () => {
  await render();
  edit(search(), "service");
  key(search(), "ArrowDown");
  key(search(), "Enter");
  edit(inputs()[0], "changed");
  expect(requestBack()).toBe(true);
  flushSync();
  expect(search().value).toBe("service");
  expect(document.activeElement).toBe(search());
  expect(host.querySelector(".picker-item.focused code")?.textContent).toBe(repeated.command);
  key(search(), "Enter");
  expect(inputs()[0].value).toBe("redis");
  key(inputs()[0], "Escape");
  key(search(), "Escape");
  expect(app.snippetPickerOpen()).toBe(false);
  expect(left.sendText).not.toHaveBeenCalled();
});

it("keeps the footer within the visible viewport and reports a disconnected target on touch submission", async () => {
  const viewport = Object.assign(new EventTarget(), { height: 700, offsetTop: 0 });
  vi.stubGlobal("visualViewport", viewport);
  const error = vi.spyOn(toast, "error").mockImplementation(() => {});
  await render();
  host.querySelectorAll<HTMLButtonElement>(".picker-item")[1].click();
  edit(inputs()[0], "redis");
  edit(inputs()[1], "30");
  Object.assign(viewport, { height: 280, offsetTop: 12 });
  viewport.dispatchEvent(new Event("resize"));
  flushSync();
  const backdrop = host.querySelector<HTMLElement>(".picker-backdrop")!;
  expect(backdrop.style.height).toBe("280px");
  expect(backdrop.style.top).toBe("12px");

  app.setTerminalConnectionStatus("left", "disconnected");
  sendButton().click();
  expect(error).toHaveBeenCalledWith("The target terminal is no longer available");
  expect(app.snippetPickerOpen()).toBe(true);
  expect(left.sendText).not.toHaveBeenCalled();
  expect(right.sendText).not.toHaveBeenCalled();
});
