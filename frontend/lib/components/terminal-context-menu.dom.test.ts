import { readFileSync } from "node:fs";
import { join } from "node:path";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { afterEach, describe, expect, it, vi } from "vitest";
import { installInputTracking, isTouchContextMenu } from "../input.ts";

// Exercise the component's real capture handler with DOM propagation, without
// starting a terminal session or substituting another implementation of it.
const source = readFileSync(join(process.cwd(), "frontend/lib/components/TerminalPane.svelte"), "utf8");
const start = source.indexOf("function onTerminalContextMenu(");
const end = source.indexOf("\n    function openSearch()", start);
if (start < 0 || end < 0) throw new Error("Terminal context-menu handler not found");
const handlerCode = ts.transpileModule(source.slice(start, end), {
  compilerOptions: { target: ts.ScriptTarget.ES2022 },
}).outputText;

type Action = "menu" | "paste" | "copyPaste";
const cleanups: Array<() => void> = [];

afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
  document.body.replaceChildren();
});

function setup({
  action = "menu", selected = false, copied = true, isMac = false, modifierMenu = true,
}: {
  action?: Action;
  selected?: boolean;
  copied?: boolean;
  isMac?: boolean;
  modifierMenu?: boolean;
} = {}) {
  const host = document.createElement("div");
  const xterm = document.createElement("div");
  const row = document.createElement("span");
  row.textContent = "terminal output";
  xterm.append(row);
  host.append(xterm);
  document.body.append(host);

  const readClipboard = vi.fn().mockResolvedValue("clipboard text");
  const pasteText = vi.fn();
  const copySelection = vi.fn().mockResolvedValue(copied);
  const clearSelection = vi.fn();
  const handler = runInNewContext(handlerCode + "\nonTerminalContextMenu", {
    isTouchContextMenu,
    app: { rightClickAction: () => action, ctrlRightClickMenu: () => modifierMenu },
    keymap: { isMac },
    terminal: { hasSelection: () => selected, clearSelection },
    readClipboard, pasteText, copySelection,
    toast: { error: vi.fn() }, errMsg: String,
  }) as (event: MouseEvent) => void;

  // This is where xterm normally moves/focuses/selects its hidden textarea.
  // The host's capture listener must prevent that handler from seeing touch.
  const xtermHandler = vi.fn();
  xterm.addEventListener("contextmenu", xtermHandler);
  host.addEventListener("contextmenu", handler, true);
  cleanups.push(installInputTracking());

  function menu(pointerType?: string, modifiers: MouseEventInit = {}) {
    const init = { bubbles: true, cancelable: true, button: 2, ...modifiers };
    const event = pointerType === undefined
      ? new MouseEvent("contextmenu", init)
      : new PointerEvent("contextmenu", { ...init, pointerType });
    row.dispatchEvent(event);
    return event;
  }

  return { row, menu, xtermHandler, readClipboard, pasteText, copySelection, clearSelection };
}

describe("terminal native touch context menu", () => {
  it.each(["touch", "pen"])("keeps the %s native menu without running xterm or clipboard actions", (pointerType) => {
    for (const action of ["menu", "paste", "copyPaste"] as const) {
      const ui = setup({ action, selected: true });
      const event = ui.menu(pointerType);

      expect(event.defaultPrevented).toBe(false);
      expect(ui.xtermHandler).not.toHaveBeenCalled();
      expect(ui.readClipboard).not.toHaveBeenCalled();
      expect(ui.copySelection).not.toHaveBeenCalled();
      expect(ui.clearSelection).not.toHaveBeenCalled();
    }
  });

  it.each(["touch", "pen"])("preserves a legacy WebView's %s menu using the preceding pointer event", (pointerType) => {
    const ui = setup({ action: "paste" });
    ui.row.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerType }));
    const event = ui.menu();

    expect(event.defaultPrevented).toBe(false);
    expect(ui.xtermHandler).not.toHaveBeenCalled();
    expect(ui.readClipboard).not.toHaveBeenCalled();
  });
});

describe("terminal mouse context menu", () => {
  it("keeps xterm's menu path after switching from touch to mouse", () => {
    const ui = setup();
    ui.row.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerType: "touch" }));
    ui.row.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerType: "mouse" }));
    const event = ui.menu();

    expect(event.defaultPrevented).toBe(false);
    expect(ui.xtermHandler).toHaveBeenCalledExactlyOnceWith(event);
    expect(ui.readClipboard).not.toHaveBeenCalled();
  });

  it.each(["paste", "copyPaste"] as const)("%s pastes without opening xterm's menu when there is no selection", async (action) => {
    const ui = setup({ action });
    const event = ui.menu("mouse");
    await Promise.resolve();

    expect(event.defaultPrevented).toBe(true);
    expect(ui.xtermHandler).not.toHaveBeenCalled();
    expect(ui.readClipboard).toHaveBeenCalledOnce();
    expect(ui.pasteText).toHaveBeenCalledExactlyOnceWith("clipboard text");
    expect(ui.copySelection).not.toHaveBeenCalled();
  });

  it.each([true, false])("copyPaste clears the selection only after a successful copy (copied=%s)", async (copied) => {
    const ui = setup({ action: "copyPaste", selected: true, copied });
    const event = ui.menu("mouse");
    await Promise.resolve();

    expect(event.defaultPrevented).toBe(true);
    expect(ui.xtermHandler).not.toHaveBeenCalled();
    expect(ui.copySelection).toHaveBeenCalledOnce();
    expect(ui.clearSelection).toHaveBeenCalledTimes(copied ? 1 : 0);
    expect(ui.readClipboard).not.toHaveBeenCalled();
  });

  it.each([
    { isMac: false, ctrlKey: true },
    { isMac: true, metaKey: true },
  ])("keeps the modifier menu escape hatch ($isMac)", ({ isMac, ...modifiers }) => {
    const ui = setup({ action: "paste", isMac });
    const event = ui.menu("mouse", modifiers);

    expect(event.defaultPrevented).toBe(false);
    expect(ui.xtermHandler).toHaveBeenCalledExactlyOnceWith(event);
    expect(ui.readClipboard).not.toHaveBeenCalled();
  });

  it("honors a disabled modifier menu preference", async () => {
    const ui = setup({ action: "paste", modifierMenu: false });
    const event = ui.menu("mouse", { ctrlKey: true });
    await Promise.resolve();

    expect(event.defaultPrevented).toBe(true);
    expect(ui.xtermHandler).not.toHaveBeenCalled();
    expect(ui.pasteText).toHaveBeenCalledExactlyOnceWith("clipboard text");
  });
});
