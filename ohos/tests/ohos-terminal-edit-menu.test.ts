import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import { Window } from "happy-dom";
import type { CustomEvent, HTMLElement, HTMLInputElement, HTMLTextAreaElement } from "happy-dom";
import ts from "typescript";
import { describe, expect, it, onTestFinished, vi } from "vitest";

class MenuId {
  constructor(readonly value: string) {}
  equals(other: MenuId): boolean { return this.value === other.value; }
  static of(value: string): MenuId { return new MenuId(value); }
  static readonly COPY = MenuId.of("copy");
  static readonly PASTE = MenuId.of("paste");
  static readonly SELECT_ALL = MenuId.of("select-all");
}

const TouchType = { Down: 0, Up: 1, Move: 2, Cancel: 3 };
interface MenuItem { id: MenuId; content: string }
interface NativeTouch { type: number; changedTouches: Array<{ x: number; y: number }>; touches: Array<{ x: number; y: number }> }
interface TerminalEditMenu {
  options: {
    onCreateMenu(items: MenuItem[]): MenuItem[];
    onPrepareMenu(items: MenuItem[]): MenuItem[];
    onMenuItemClick(item: MenuItem, range: { start: number; end: number }): boolean;
  };
  onTouch(event: NativeTouch): void;
  reset(): void;
}

const sourcePath = fileURLToPath(new URL(
  "../app/common/runtime/src/main/ets/ability/TerminalEditMenu.ets", import.meta.url,
));
const copy: MenuItem = { id: MenuId.COPY, content: "Copy" };
const selectAll: MenuItem = { id: MenuId.SELECT_ALL, content: "Select all" };
const systemPaste: MenuItem = { id: MenuId.PASTE, content: "Paste" };
const range = { start: -1, end: -1 };

async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

function harness() {
  const window = new Window({ url: "https://tauri.localhost" });
  onTestFinished(() => window.happyDOM.abort());
  const document = window.document;
  Object.defineProperty(window, "devicePixelRatio", { value: 3 });
  document.body.innerHTML = `
    <div class="xterm-host" data-terminal-session="session-a">
      <div class="xterm"><div class="xterm-rows"><span>terminal output</span></div></div>
      <textarea></textarea>
    </div>
    <input value="ordinary form input">
    <div class="other-output">ordinary page text</div>
    <div class="xterm-host" data-terminal-session="session-b"><span>second terminal</span></div>`;
  const host = document.querySelector<HTMLElement>(".xterm-host")!;
  const row = host.querySelector<HTMLElement>("span")!;
  const textarea = host.querySelector<HTMLTextAreaElement>("textarea")!;
  const input = document.querySelector<HTMLInputElement>("input")!;
  const other = document.querySelector<HTMLElement>(".other-output")!;
  const secondHost = document.querySelector<HTMLElement>('[data-terminal-session="session-b"]')!;
  const secondRow = secondHost.querySelector<HTMLElement>("span")!;

  // Happy DOM has no layout engine. Supply only geometry/hit testing; selection,
  // ancestor queries, visibility, and dispatched DOM events remain real.
  function geometry(element: HTMLElement, x: number, y: number, width: number, height: number) {
    const rect = new window.DOMRect(x, y, width, height);
    element.getBoundingClientRect = () => element.isConnected && element.style.display !== "none"
      ? rect : new window.DOMRect();
    element.getClientRects = () => {
      const current = element.getBoundingClientRect();
      return (current.width && current.height ? [current] : []) as ReturnType<HTMLElement["getClientRects"]>;
    };
    return rect;
  }
  geometry(host, 0, 0, 300, 200);
  const rowRect = geometry(row, 10, 30, 140, 20);
  geometry(textarea, 0, 190, 10, 10);
  geometry(input, 0, 240, 300, 20);
  geometry(other, 0, 300, 300, 20);
  geometry(secondHost, 0, 400, 300, 100);
  geometry(secondRow, 10, 420, 140, 20);
  const elementFromPoint = vi.fn((x: number, y: number) => {
    for (const element of [row, textarea, secondRow, host, input, other, secondHost]) {
      const rect = element.getBoundingClientRect();
      if (rect.width && rect.height && x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom) {
        return element;
      }
    }
    return document.body;
  });
  document.elementFromPoint = elementFromPoint;

  const mimeTypes = new Set(["text/plain"]);
  const getData = vi.fn(() => { throw new Error("The menu must not read clipboard contents"); });
  const hasDataType = vi.fn((type: string) => mimeTypes.has(type));
  const pasteboard = {
    MIMETYPE_TEXT_PLAIN: "text/plain", MIMETYPE_TEXT_HTML: "text/html",
    getSystemPasteboard: () => ({ hasDataType, getData }),
  };
  const evaluate = (script: string): string => JSON.stringify(window.eval(script)) ?? "null";
  const runJavaScript = vi.fn(async (script: string): Promise<string> => evaluate(script));
  const modules: Record<string, unknown> = {
    "@ohos.pasteboard": { default: pasteboard },
    "@kit.BasicServicesKit": { pasteboard },
    "@ohos.web.webview": { default: {} },
    "@kit.ArkWeb": { webview: {} },
    "@kit.ArkUI": { TextMenuItemId: MenuId, TouchType },
  };
  const exports: Record<string, unknown> = {};
  const code = ts.transpileModule(readFileSync(sourcePath, "utf8"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  runInNewContext(code, {
    exports, TextMenuItemId: MenuId, TouchType, $r: (name: string) => name,
    console: { warn: vi.fn(), error: vi.fn(), info: vi.fn() },
    require: (name: string) => {
      if (!(name in modules)) throw new Error(`Unexpected platform dependency: ${name}`);
      return modules[name];
    },
  }, { filename: sourcePath });
  const Constructor = exports.TerminalEditMenu as new (controller: { runJavaScript: typeof runJavaScript }) => TerminalEditMenu;
  const menu = new Constructor({ runJavaScript });

  function touch(x = 20, y = 40, type = TouchType.Down) {
    const point = { x, y };
    menu.onTouch({ type, changedTouches: [point], touches: [point] });
    return flush();
  }
  function selectOutput() {
    const selected = document.createRange();
    selected.selectNodeContents(row);
    selected.getBoundingClientRect = () => rowRect;
    selected.getClientRects = () => [rowRect] as ReturnType<HTMLElement["getClientRects"]>;
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(selected);
  }
  function paste(items = menu.options.onCreateMenu([copy, selectAll])) {
    const item = items.find((candidate) => candidate.id.equals(MenuId.PASTE));
    expect(item).toBeDefined();
    return item!;
  }
  function listen(element = host, session = element.dataset.terminalSession!) {
    const events: unknown[] = [];
    element.addEventListener(`terminal:paste:${session}`, (event) => events.push((event as CustomEvent).detail));
    return events;
  }
  function delayNextResult() {
    let release!: () => void;
    runJavaScript.mockImplementationOnce((script) => {
      const result = evaluate(script);
      return new Promise<string>((resolve) => { release = () => resolve(result); });
    });
    return async () => { release(); await flush(); };
  }
  return { menu, window, document, host, row, textarea, input, secondHost, touch, selectOutput, paste, listen,
    runJavaScript, elementFromPoint, mimeTypes, getData, hasDataType, delayNextResult };
}

describe("HarmonyOS terminal edit menu", () => {
  it("adds system Paste on the first terminal long press without reading clipboard contents", async () => {
    const h = harness();
    expect(h.window.getSelection()!.toString()).toBe("");
    await h.touch();
    const items = h.menu.options.onCreateMenu([copy, selectAll]);
    expect(items).toContain(copy);
    expect(items).toContain(selectAll);
    expect(items.filter((item) => item.id.equals(MenuId.PASTE))).toHaveLength(1);
    // Native local touch coordinates are vp, equivalent to CSS pixels at scale 1.
    expect(h.elementFromPoint).toHaveBeenCalledWith(20, 40);
    expect(h.getData).not.toHaveBeenCalled();
  });

  it.each([{ types: [] }, { types: ["image/png"] }])("keeps the normal menu for non-text clipboard types $types", async ({ types }) => {
    const h = harness();
    h.mimeTypes.clear();
    for (const type of types) h.mimeTypes.add(type);
    await h.touch();
    expect(h.menu.options.onCreateMenu([copy, selectAll])).toEqual([copy, selectAll]);
    expect(h.getData).not.toHaveBeenCalled();
  });

  it("accepts HTML clipboard content and does not duplicate Paste when preparing the menu", async () => {
    const h = harness();
    h.mimeTypes.clear();
    h.mimeTypes.add("text/html");
    await h.touch();
    const items = h.menu.options.onCreateMenu([copy, selectAll]);
    const prepared = h.menu.options.onPrepareMenu(items);
    expect(prepared.filter((item) => item.id.equals(MenuId.PASTE))).toHaveLength(1);
    const events = h.listen();
    expect(h.menu.options.onMenuItemClick(h.paste(prepared), range)).toBe(true);
    await flush();
    expect(events).toEqual([{ readClipboard: true }]);
  });

  it.each([[20, 250], [20, 310], [5, 195]])("preserves form and other-page menus at (%s, %s)", async (x, y) => {
    const h = harness();
    await h.touch(x, y);
    const original = [copy, systemPaste, selectAll];
    expect(h.menu.options.onCreateMenu(original)).toEqual(original);
    expect(h.menu.options.onMenuItemClick(systemPaste, range)).toBe(false);
    expect(h.menu.options.onCreateMenu([copy])).toEqual([copy]);
    expect(h.getData).not.toHaveBeenCalled();
  });

  it("preserves an existing native Paste action even after touching terminal output", async () => {
    const h = harness();
    await h.touch();
    const items = h.menu.options.onCreateMenu([copy, systemPaste]);
    const calls = h.runJavaScript.mock.calls.length;
    expect(items).toEqual([copy, systemPaste]);
    expect(h.menu.options.onMenuItemClick(systemPaste, range)).toBe(false);
    expect(h.runJavaScript).toHaveBeenCalledTimes(calls);
  });

  it("does not add a terminal action to editable descendants", async () => {
    const h = harness();
    h.row.setAttribute("contenteditable", "true");
    await h.touch();
    expect(h.menu.options.onCreateMenu([copy])).toEqual([copy]);
    expect(h.menu.options.onMenuItemClick(systemPaste, range)).toBe(false);
  });

  it("does not cache hidden terminal output as a paste target", async () => {
    const h = harness();
    h.host.style.display = "none";
    await h.touch();
    expect(h.menu.options.onCreateMenu([copy])).toEqual([copy]);
  });

  it("leaves the normal menu intact when clipboard metadata cannot be read", async () => {
    const h = harness();
    h.hasDataType.mockImplementation(() => { throw new Error("pasteboard unavailable"); });
    await h.touch();
    expect(h.menu.options.onCreateMenu([copy, selectAll])).toEqual([copy, selectAll]);
    expect(h.getData).not.toHaveBeenCalled();
  });

  it("ignores an older hit-test result after a newer terminal touch", async () => {
    const h = harness();
    const release = h.delayNextResult();
    await h.touch();
    await h.touch(20, 430);
    await release();
    const first = h.listen();
    const second = h.listen(h.secondHost);
    expect(h.menu.options.onMenuItemClick(h.paste(), range)).toBe(true);
    await flush();
    expect(first).toEqual([]);
    expect(second).toEqual([{ readClipboard: true }]);
  });

  it("invalidates pending hit tests and existing menu actions when the page resets", async () => {
    const h = harness();
    await h.touch();
    const paste = h.paste();
    const release = h.delayNextResult();
    await h.touch();
    h.menu.reset();
    await release();
    const events = h.listen();
    expect(h.menu.options.onCreateMenu([copy])).toEqual([copy]);
    h.menu.options.onMenuItemClick(paste, range);
    await flush();
    expect(events).toEqual([]);
  });

  it("dispatches one clipboard-read request to the captured session after selection collapses", async () => {
    const h = harness();
    h.host.dataset.terminalSession = 'session-"\\\n';
    h.selectOutput();
    await h.touch();
    const paste = h.paste();
    const events = h.listen();
    h.window.getSelection()!.removeAllRanges();
    expect(h.menu.options.onMenuItemClick(paste, range)).toBe(true);
    await flush();
    expect(events).toEqual([{ readClipboard: true }]);
    expect(h.getData).not.toHaveBeenCalled();
    expect(h.menu.options.onMenuItemClick(paste, range)).toBe(false);
    await flush();
    expect(events).toHaveLength(1);
  });

  it.each(["hidden", "detached", "reconnected"])("does not dispatch to a %s terminal after the menu opens", async (state) => {
    const h = harness();
    await h.touch();
    const paste = h.paste();
    const events = h.listen();
    if (state === "hidden") h.host.style.display = "none";
    if (state === "detached") h.host.remove();
    if (state === "reconnected") h.host.dataset.terminalSession = "replacement-session";
    h.menu.options.onMenuItemClick(paste, range);
    await flush();
    expect(events).toEqual([]);
  });

  it.each([".other-output", "input"])("does not paste when the open menu's selection moves to %s", async (selector) => {
    const h = harness();
    h.selectOutput();
    await h.touch();
    const paste = h.paste();
    const events = h.listen();
    const target = h.document.querySelector(selector)!;
    const selected = h.document.createRange();
    if (selector === "input") selected.selectNode(target);
    else selected.selectNodeContents(target);
    h.window.getSelection()!.removeAllRanges();
    h.window.getSelection()!.addRange(selected);
    expect(h.window.getSelection()!.isCollapsed).toBe(false);
    expect(h.host.contains(h.window.getSelection()!.anchorNode)).toBe(false);

    h.menu.options.onMenuItemClick(paste, range);
    await flush();
    expect(events).toEqual([]);
  });

  it("keeps a valid terminal selection when a handle touch lands just outside its host", async () => {
    const h = harness();
    await h.touch();
    h.selectOutput();
    await h.touch(-3, 40);
    const events = h.listen();
    h.menu.options.onMenuItemClick(h.paste(), range);
    await flush();
    expect(events).toEqual([{ readClipboard: true }]);
  });

  it("does not borrow the nearby terminal selection when touching a button", async () => {
    const h = harness();
    h.selectOutput();
    const button = h.document.createElement("button");
    button.innerHTML = "<span>Open menu</span>";
    h.document.body.append(button);
    h.elementFromPoint.mockReturnValueOnce(button.querySelector<HTMLElement>("span")!);
    await h.touch(-3, 40);

    expect(h.menu.options.onCreateMenu([copy])).toEqual([copy]);
    expect(h.menu.options.onMenuItemClick(systemPaste, range)).toBe(false);
  });

  it("does not use a selection spanning different terminals as a handle fallback", async () => {
    const h = harness();
    const selected = h.document.createRange();
    selected.setStart(h.row.firstChild!, 0);
    selected.setEnd(h.secondHost.querySelector("span")!.firstChild!, 4);
    h.window.getSelection()!.addRange(selected);
    await h.touch(-3, 40);
    expect(h.menu.options.onCreateMenu([copy])).toEqual([copy]);
  });

  it.each([[20, 40], [-3, 40]])("retains ownership of its Paste action when a handle reopens the menu at (%s, %s)", async (x, y) => {
    const h = harness();
    await h.touch();
    const items = h.menu.options.onCreateMenu([copy, selectAll]);
    h.selectOutput();
    await h.touch(x, y);
    const prepared = h.menu.options.onPrepareMenu(items);
    expect(prepared.filter((item) => item.id.equals(MenuId.PASTE))).toHaveLength(1);
    const events = h.listen();
    expect(h.menu.options.onMenuItemClick(h.paste(prepared), range)).toBe(true);
    await flush();
    expect(events).toEqual([{ readClipboard: true }]);

    await h.touch(20, 250);
    h.menu.options.onCreateMenu([copy, systemPaste]);
    expect(h.menu.options.onMenuItemClick(systemPaste, range)).toBe(false);
    expect(events).toHaveLength(1);
  });

  it("does not replace the target while dragging, releasing, or cancelling a selection handle", async () => {
    const h = harness();
    await h.touch();
    const calls = h.runJavaScript.mock.calls.length;
    for (const type of [TouchType.Move, TouchType.Up, TouchType.Cancel]) await h.touch(20, 250, type);
    expect(h.runJavaScript).toHaveBeenCalledTimes(calls);
    const events = h.listen();
    h.menu.options.onMenuItemClick(h.paste(), range);
    await flush();
    expect(events).toEqual([{ readClipboard: true }]);
  });
});
