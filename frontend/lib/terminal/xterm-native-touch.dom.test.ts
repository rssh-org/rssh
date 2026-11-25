import type { Terminal } from "@xterm/xterm";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setupXtermNativeTouch } from "./xterm-native-touch.ts";

const cleanups: Array<() => void> = [];

beforeEach(() => {
  vi.spyOn(navigator, "maxTouchPoints", "get").mockReturnValue(1);
});

afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
  window.getSelection()?.removeAllRanges();
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

function setup({ enabled = true, textareaAvailable = true } = {}) {
  const host = document.createElement("div");
  host.className = "xterm-host";
  host.innerHTML = `<div class="xterm"><div class="xterm-screen">
    <div class="xterm-rows"><div><span>terminal output</span><span class="xterm-cursor"> </span></div></div>
    <div class="xterm-helpers"><textarea></textarea></div>
  </div></div>`;
  document.body.append(host);
  const element = host.querySelector<HTMLElement>(".xterm")!;
  const screen = host.querySelector<HTMLElement>(".xterm-screen")!;
  const text = host.querySelector<HTMLElement>(".xterm-rows span")!;
  const textarea = host.querySelector("textarea")!;
  vi.spyOn(screen, "getBoundingClientRect").mockReturnValue(new DOMRect(20, 40, 800, 480));
  const listeners = {
    render: new Set<() => void>(),
    scroll: new Set<() => void>(),
    cursor: new Set<() => void>(),
  };
  const subscribe = (set: Set<() => void>) => (listener: () => void) => {
    set.add(listener);
    return { dispose: () => set.delete(listener) };
  };
  const emit = (event: keyof typeof listeners) => {
    for (const listener of [...listeners[event]]) listener();
  };
  const selection = { active: false };
  const terminal = {
    element,
    textarea: textareaAvailable ? textarea : undefined,
    modes: { mouseTrackingMode: "none" },
    buffer: { active: { baseY: 20, viewportY: 20, cursorX: 5, cursorY: 3 } },
    cols: 80,
    rows: 24,
    hasSelection: () => selection.active,
    clearSelection: vi.fn(() => {
      selection.active = false;
      // xterm redraws rows when clearing its selection, replacing hit targets.
      text.replaceWith(text.cloneNode(true));
      emit("render");
    }),
    onRender: subscribe(listeners.render),
    onScroll: subscribe(listeners.scroll),
    onCursorMove: subscribe(listeners.cursor),
  };
  // A short tap reaches xterm's existing mouse path and restores input focus.
  // Native long presses are handled by WebKit without this compatibility event.
  const xtermMouseDown = vi.fn(() => textarea.focus());
  element.addEventListener("mousedown", xtermMouseDown);
  const xtermCopy = vi.fn((event: Event) => {
    if (!selection.active) return;
    (event as ClipboardEvent).clipboardData?.setData("text/plain", "Second");
    event.preventDefault();
  });
  element.addEventListener("copy", xtermCopy);
  const cleanup = setupXtermNativeTouch({
    terminal: terminal as unknown as Terminal, host, enabled,
  });
  cleanups.push(cleanup);
  const pointer = (pointerType: string, type = "pointerdown", target: HTMLElement = text) => {
    const event = new PointerEvent(type, { bubbles: true, cancelable: true, pointerType });
    target.dispatchEvent(event);
    return event;
  };
  const mouseDown = () => {
    const event = new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 });
    text.dispatchEvent(event);
    return event;
  };
  return { host, element, screen, text, textarea, terminal, selection, listeners, emit, pointer, mouseDown, xtermMouseDown, xtermCopy, cleanup };
}

function copy(target: HTMLElement) {
  const event = new Event("copy", { bubbles: true, cancelable: true });
  const setData = vi.fn();
  Object.defineProperty(event, "clipboardData", { value: { setData } });
  target.dispatchEvent(event);
  return { event, setData };
}

function paste(target: HTMLElement, clipboardData: Pick<DataTransfer, "getData"> | null = { getData: () => "pasted text" }) {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", { value: clipboardData });
  target.dispatchEvent(event);
  return event;
}

describe("xterm native touch integration", () => {
  it.each(["pointerover", "pointerdown"])("switches the native paste affordance on %s without intercepting xterm mouse input", (type) => {
    const ui = setup();
    expect(ui.host.classList.contains("xterm-native-touch")).toBe(true);
    expect(ui.mouseDown().defaultPrevented).toBe(false);
    expect(ui.xtermMouseDown).toHaveBeenCalledOnce();
    expect(document.activeElement).toBe(ui.textarea);

    ui.pointer("mouse", type);
    expect(ui.host.classList.contains("xterm-native-touch")).toBe(false);
    ui.mouseDown();
    expect(ui.xtermMouseDown).toHaveBeenCalledTimes(2);

    ui.pointer("touch");
    expect(ui.host.classList.contains("xterm-native-touch")).toBe(true);
    expect(ui.mouseDown().defaultPrevented).toBe(false);
    expect(ui.xtermMouseDown).toHaveBeenCalledTimes(3);
  });

  it("enables pen input after starting without touch hardware", () => {
    vi.spyOn(navigator, "maxTouchPoints", "get").mockReturnValue(0);
    const ui = setup();
    expect(ui.host.classList.contains("xterm-native-touch")).toBe(false);
    ui.pointer("pen");
    expect(ui.host.classList.contains("xterm-native-touch")).toBe(true);
    expect(ui.mouseDown().defaultPrevented).toBe(false);
    expect(ui.xtermMouseDown).toHaveBeenCalledOnce();
  });

  it("preserves application mouse reporting and restores native paste when reporting stops", () => {
    const ui = setup();
    ui.terminal.modes.mouseTrackingMode = "drag";
    ui.pointer("touch");
    expect(ui.host.classList.contains("xterm-native-touch")).toBe(false);
    ui.mouseDown();
    expect(ui.xtermMouseDown).toHaveBeenCalledOnce();

    ui.terminal.modes.mouseTrackingMode = "none";
    ui.emit("render");
    expect(ui.host.classList.contains("xterm-native-touch")).toBe(true);
    expect(ui.mouseDown().defaultPrevented).toBe(false);
    expect(ui.xtermMouseDown).toHaveBeenCalledTimes(2);
  });

  it("keeps xterm handling input when no selectable DOM renderer is present", () => {
    const ui = setup();
    const rows = ui.host.querySelector(".xterm-rows")!;
    rows.replaceWith(document.createElement("canvas"));
    ui.emit("render");
    ui.pointer("touch", "pointerdown", ui.screen);
    expect(ui.host.classList.contains("xterm-native-touch")).toBe(false);
    expect(ui.host.classList.contains("xterm-native-cursor")).toBe(false);
    ui.screen.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    expect(ui.xtermMouseDown).toHaveBeenCalledOnce();
  });

  it("leaves pointer events available to xterm's touch scrollbar", () => {
    const ui = setup();
    const scrollbar = document.createElement("div");
    scrollbar.className = "scrollbar";
    ui.element.append(scrollbar);
    const onPointerDown = vi.fn();
    scrollbar.addEventListener("pointerdown", onPointerDown);
    const event = new PointerEvent("pointerdown", {
      bubbles: true, cancelable: true, pointerType: "touch",
    });
    scrollbar.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(onPointerDown).toHaveBeenCalledOnce();
  });

  it("preserves the hit node and native range on touch even when xterm has an older selection", () => {
    const ui = setup();
    ui.pointer("mouse");
    ui.selection.active = true;
    const range = document.createRange();
    range.selectNodeContents(ui.text);
    const selection = window.getSelection()!;
    selection.addRange(range);
    const onPointerDown = vi.fn();
    ui.text.addEventListener("pointerdown", onPointerDown);
    const event = ui.pointer("touch");
    ui.emit("render");
    expect(event.defaultPrevented).toBe(false);
    expect(onPointerDown).toHaveBeenCalledOnce();
    expect(event.target).toBe(ui.text);
    expect(ui.text.isConnected).toBe(true);
    expect(ui.terminal.clearSelection).not.toHaveBeenCalled();
    expect(selection.toString()).toBe("terminal output");
    expect(selection.anchorNode).toBe(range.startContainer);
  });

  it("cancels native textarea insertion while allowing xterm to handle paste exactly once", () => {
    const ui = setup();
    ui.textarea.value = "existing composition";
    const atTextarea = vi.fn((event: Event) => {
      expect(event.defaultPrevented).toBe(true);
      // xterm owns delivery to onData and stops its ancestor paste listener.
      event.stopPropagation();
    });
    const atXterm = vi.fn();
    ui.textarea.addEventListener("paste", atTextarea);
    ui.element.addEventListener("paste", atXterm);

    expect(paste(ui.textarea).defaultPrevented).toBe(true);
    expect(atTextarea).toHaveBeenCalledOnce();
    expect(atXterm).not.toHaveBeenCalled();
    expect(ui.textarea.value).toBe("existing composition");
  });

  it("leaves native Copy available without overwriting it with an older xterm selection", () => {
    const ui = setup();
    ui.selection.active = true;
    ui.textarea.focus();
    ui.text.textContent = "TERMCOPY";
    const range = document.createRange();
    range.selectNodeContents(ui.text);
    window.getSelection()!.addRange(range);

    // Copy is dispatched at the focused helper while the native range is in output.
    const result = copy(ui.textarea);
    expect(ui.xtermCopy).not.toHaveBeenCalled();
    expect(result.setData).not.toHaveBeenCalled();
    expect(result.event.defaultPrevented).toBe(false);
    expect(window.getSelection()!.toString()).toBe("TERMCOPY");
    expect(ui.text.isConnected).toBe(true);
    expect(ui.terminal.clearSelection).not.toHaveBeenCalled();
  });

  it.each(["empty", "collapsed", "other terminal", "start outside", "end outside"])(
    "preserves xterm Copy when the native range is %s", (kind) => {
      const ui = setup();
      ui.selection.active = true;
      if (kind !== "empty") {
        const range = document.createRange();
        range.selectNodeContents(ui.text);
        if (kind === "collapsed") {
          range.collapse();
        } else {
          const other = setup();
          if (kind === "other terminal") range.selectNodeContents(other.text);
          if (kind === "start outside") {
            ui.host.before(other.host);
            range.setStart(other.text.firstChild!, 0);
          }
          if (kind === "end outside") range.setEnd(other.text.firstChild!, 4);
        }
        window.getSelection()!.addRange(range);
      }

      const result = copy(ui.textarea);
      expect(ui.xtermCopy).toHaveBeenCalledOnce();
      expect(result.setData).toHaveBeenCalledExactlyOnceWith("text/plain", "Second");
      expect(result.event.defaultPrevented).toBe(true);
    },
  );

  it("does not cancel another input's paste or a helper paste without clipboard data", () => {
    const ui = setup();
    const otherInput = document.createElement("textarea");
    ui.host.append(otherInput);
    const targetHandler = vi.fn();
    otherInput.addEventListener("paste", targetHandler);
    expect(paste(otherInput).defaultPrevented).toBe(false);
    expect(targetHandler).toHaveBeenCalledOnce();
    expect(paste(ui.textarea, null).defaultPrevented).toBe(false);
  });

  it("only exposes the helper at the live cursor, not over scrollback or a hidden cursor", () => {
    const ui = setup();
    expect(ui.host.classList.contains("xterm-native-cursor")).toBe(true);
    const initialLeft = ui.host.style.getPropertyValue("--native-cursor-left");
    const initialTop = ui.host.style.getPropertyValue("--native-cursor-top");
    expect(initialLeft).toBe("50px");
    expect(initialTop).toBe("60px");
    ui.terminal.buffer.active.cursorX += 2;
    ui.terminal.buffer.active.cursorY += 1;
    ui.emit("cursor");
    expect(ui.host.style.getPropertyValue("--native-cursor-left")).not.toBe(initialLeft);
    expect(ui.host.style.getPropertyValue("--native-cursor-top")).not.toBe(initialTop);

    ui.terminal.buffer.active.viewportY = 10;
    ui.emit("scroll");
    expect(ui.host.classList.contains("xterm-native-cursor")).toBe(false);
    ui.terminal.buffer.active.viewportY = 20;
    ui.emit("scroll");
    expect(ui.host.classList.contains("xterm-native-cursor")).toBe(true);
    ui.host.querySelector(".xterm-cursor")!.remove();
    ui.emit("render");
    expect(ui.host.classList.contains("xterm-native-cursor")).toBe(false);
  });

  it.each([{ enabled: false }, { textareaAvailable: false }])("leaves ordinary behavior unchanged when unavailable: %o", (options) => {
    const ui = setup(options);
    ui.pointer("touch");
    expect(ui.host.classList.contains("xterm-native-touch")).toBe(false);
    expect(ui.host.classList.contains("xterm-native-cursor")).toBe(false);
    ui.mouseDown();
    expect(ui.xtermMouseDown).toHaveBeenCalledOnce();
    expect(paste(ui.textarea).defaultPrevented).toBe(false);
  });

  it("removes event interception, rendering subscriptions and styles when disposed", () => {
    const ui = setup();
    ui.selection.active = true;
    const range = document.createRange();
    range.selectNodeContents(ui.text);
    window.getSelection()!.addRange(range);
    ui.cleanup();
    ui.pointer("touch");
    ui.emit("cursor");
    expect(ui.host.className).toBe("xterm-host");
    expect(ui.host.style.getPropertyValue("--native-cursor-left")).toBe("");
    expect(ui.host.style.getPropertyValue("--native-cursor-top")).toBe("");
    expect(Object.values(ui.listeners).every(set => set.size === 0)).toBe(true);
    ui.mouseDown();
    expect(ui.xtermMouseDown).toHaveBeenCalledOnce();
    expect(paste(ui.textarea).defaultPrevented).toBe(false);
    expect(window.getSelection()!.toString()).toBe("terminal output");
    const result = copy(ui.textarea);
    expect(ui.xtermCopy).toHaveBeenCalledOnce();
    expect(result.setData).toHaveBeenCalledExactlyOnceWith("text/plain", "Second");
    expect(result.event.defaultPrevented).toBe(true);
  });
});
