import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setupTerminalSoftKeyboard } from "./soft-keyboard.ts";

class FakeElement extends EventTarget {
  readOnly = false;
  inert = false;
  style = { removeProperty: vi.fn() };
  attributes = new Map<string, string>();
  getAttribute(name: string) { return this.attributes.get(name) ?? null; }
  setAttribute(name: string, value: string) { this.attributes.set(name, value); }
  removeAttribute(name: string) { this.attributes.delete(name); }
  getBoundingClientRect() { return { top: 0, bottom: 500 }; }
  focus() {
    if (document.activeElement === this as unknown as Element) return;
    Object.assign(document, { activeElement: this });
    this.dispatchEvent(new Event("focus"));
  }
  blur() {
    if (document.activeElement !== this as unknown as Element) return;
    Object.assign(document, { activeElement: null });
    this.dispatchEvent(new Event("blur"));
  }
}

function pointer(target: EventTarget, pointerType: string) {
  const event = new Event("pointerdown");
  Object.defineProperty(event, "pointerType", { value: pointerType });
  target.dispatchEvent(event);
}

/** Node EventTarget has no DOM tree. Dispatch the ancestor capture phase before
 * the target phase explicitly, preserving the original textarea event target. */
function dispatchInput(host: EventTarget, helper: EventTarget, type: string): Event {
  const event = new Event(type, { cancelable: true });
  Object.defineProperty(event, "target", { value: helper });
  host.dispatchEvent(event);
  helper.dispatchEvent(event);
  return event;
}

describe("terminal soft keyboard", () => {
  let dispose: (() => void) | undefined;

  beforeEach(() => {
    vi.stubGlobal("window", Object.assign(new EventTarget(), { innerHeight: 1000 }));
    vi.stubGlobal("document", {
      activeElement: null,
      documentElement: { scrollTop: 0, scrollLeft: 0 },
      body: { scrollTop: 0, scrollLeft: 0 },
    });
    vi.stubGlobal("requestAnimationFrame", vi.fn(() => 1));
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
  });

  afterEach(() => { dispose?.(); vi.unstubAllGlobals(); });

  function setup(touchAvailable: boolean, beforeInstall?: (helper: FakeElement) => void) {
    const helper = new FakeElement();
    const host = new FakeElement();
    const onRequestedChange = vi.fn();
    helper.setAttribute("style", "left: 120px; top: 40px;");
    beforeInstall?.(helper);
    const controller = setupTerminalSoftKeyboard({
      helper: helper as unknown as HTMLTextAreaElement,
      host: host as unknown as HTMLElement,
      pane: new FakeElement() as unknown as HTMLElement,
      touchAvailable,
      onRequestedChange,
    });
    dispose = controller.dispose;
    return { helper, host, onRequestedChange, controller };
  }

  it("ordinary desktop focus neither locks input nor requests a soft keyboard", () => {
    const { helper, onRequestedChange } = setup(false);
    helper.focus();
    expect(helper.readOnly).toBe(false);
    expect(onRequestedChange).not.toHaveBeenCalled();
  });

  it("unlocks on host capture before xterm's earlier textarea capture handler", () => {
    let readOnlyAtXterm: boolean | undefined;
    const { helper, host, onRequestedChange } = setup(true, (textarea) => {
      // xterm registers its target capture handler before the keyboard adapter.
      textarea.addEventListener("keydown", () => { readOnlyAtXterm = textarea.readOnly; }, true);
    });
    expect(helper.readOnly).toBe(true);
    expect(helper.inert).toBe(false);
    helper.focus();
    expect(document.activeElement).toBe(helper);
    const event = dispatchInput(host, helper, "keydown");
    expect(readOnlyAtXterm).toBe(false);
    expect(event.defaultPrevented).toBe(false);
    expect(onRequestedChange).not.toHaveBeenCalled();
  });

  it("lets hardware IME start composing after touch input", () => {
    const { helper, host } = setup(false);
    pointer(host, "touch");
    expect(helper.readOnly).toBe(true);
    dispatchInput(host, helper, "compositionstart");
    expect(helper.readOnly).toBe(false);
  });

  it("ignores keyboard events targeting a different input", () => {
    const { helper, host } = setup(true);
    dispatchInput(host, new FakeElement(), "keydown");
    expect(helper.readOnly).toBe(true);
  });

  it("tracks explicit requests rather than focus and restores mouse input geometry", () => {
    const { helper, host, controller, onRequestedChange } = setup(true);
    helper.focus();
    controller.toggle();
    expect(onRequestedChange).toHaveBeenLastCalledWith(true);
    expect(helper.readOnly).toBe(false);
    pointer(host, "mouse");
    expect(onRequestedChange).toHaveBeenLastCalledWith(false);
    expect(helper.readOnly).toBe(false);
    expect(helper.getAttribute("style")).toBe("left: 120px; top: 40px;");
    controller.toggle();
    expect(onRequestedChange).toHaveBeenLastCalledWith(true);
  });

  it("touch/pen dismissal and leaving the pane clear the explicit request", () => {
    const { helper, host, controller, onRequestedChange } = setup(false);
    controller.toggle();
    pointer(host, "pen");
    expect(onRequestedChange).toHaveBeenLastCalledWith(false);
    expect(helper.readOnly).toBe(true);
    controller.toggle();
    controller.hide();
    expect(onRequestedChange).toHaveBeenLastCalledWith(false);
    expect(document.activeElement).not.toBe(helper);
  });
});
