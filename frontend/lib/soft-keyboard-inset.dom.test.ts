import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setupSoftKeyboardInset } from "./soft-keyboard-inset.ts";

let resizePanel: (() => void) | undefined;
let resizeContainer: (() => void) | undefined;
const disconnect = vi.fn();
let cleanup: (() => void) | undefined;
let container: HTMLDivElement;
let panel: HTMLDivElement;
let input: HTMLTextAreaElement;
let top: number;
let bottom: number;
let viewport: EventTarget & { offsetTop: number; height: number };

beforeEach(() => {
  top = 0;
  bottom = 800;
  resizePanel = undefined;
  resizeContainer = undefined;
  disconnect.mockClear();
  viewport = Object.assign(new EventTarget(), { offsetTop: 0, height: 500 });
  vi.stubGlobal("visualViewport", viewport);
  vi.stubGlobal("ResizeObserver", class {
    targets = new Set<Element>();
    constructor(callback: () => void) {
      resizePanel = () => { if (this.targets.has(panel)) callback(); };
      resizeContainer = () => { if (this.targets.has(container)) callback(); };
    }
    observe(target: Element) { this.targets.add(target); }
    disconnect = disconnect;
  });
  container = document.createElement("div");
  panel = document.createElement("div");
  panel.style.boxSizing = "border-box";
  panel.style.height = "100%";
  input = document.createElement("textarea");
  panel.append(input);
  container.append(panel);
  document.body.append(container);
  vi.spyOn(container, "clientHeight", "get").mockImplementation(() => bottom - top);
  vi.spyOn(panel, "getBoundingClientRect").mockImplementation(() => {
    const style = getComputedStyle(panel);
    // A border-box cannot be smaller than its padding and borders, even
    // when height: 100% asks it to fit a smaller parent.
    const minimumHeight = [style.paddingTop, style.paddingBottom,
      style.borderTopWidth, style.borderBottomWidth]
      .reduce((sum, value) => sum + (parseFloat(value) || 0), 0);
    const height = Math.max(bottom - top, minimumHeight);
    return { top, height, bottom: top + height } as DOMRect;
  });
  cleanup = setupSoftKeyboardInset(panel, input);
});

afterEach(() => {
  cleanup?.();
  cleanup = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("soft keyboard inset follows panel layout", () => {
  it("does not grow a split pane entirely below the keyboard on repeated resize callbacks", () => {
    top = 480;
    viewport.height = 300;
    input.focus();
    expect(panel.style.paddingBottom).toBe("320px");
    expect(panel.getBoundingClientRect().bottom).toBe(800);

    const mutations = new MutationObserver(() => {});
    mutations.observe(panel, { attributes: true, attributeFilter: ["style"] });
    for (let i = 0; i < 5; i++) resizePanel?.();
    expect(mutations.takeRecords()).toHaveLength(0);
    expect(panel.getBoundingClientRect().bottom).toBe(800);
    mutations.disconnect();
  });

  it("reserves the pane's top padding and borders when clamping the inset", () => {
    top = 480;
    viewport.height = 300;
    panel.style.paddingTop = "24px";
    panel.style.borderTop = "2px solid black";
    panel.style.borderBottom = "3px solid black";
    input.focus();
    expect(panel.style.paddingBottom).toBe("291px");
    resizePanel?.();
    expect(panel.style.paddingBottom).toBe("291px");
    expect(panel.getBoundingClientRect().height).toBe(320);
  });

  it("shrinks an existing inset with the parent's available height and clears it on recovery", () => {
    top = 480;
    viewport.height = 300;
    input.focus();
    bottom = 680;
    resizeContainer?.();
    expect(panel.style.paddingBottom).toBe("200px");
    expect(panel.getBoundingClientRect().bottom).toBe(680);

    bottom = 800;
    resizeContainer?.();
    expect(panel.style.paddingBottom).toBe("320px");
    viewport.height = 700;
    viewport.dispatchEvent(new Event("resize"));
    expect(panel.style.paddingBottom).toBe("100px");
    viewport.height = 800;
    viewport.dispatchEvent(new Event("resize"));
    expect(panel.style.paddingBottom).toBe("");
  });

  it("limits a smaller pane to its own height instead of filling its parent", () => {
    top = 600;
    panel.style.height = "200px";
    vi.spyOn(container, "clientHeight", "get").mockReturnValue(800);
    input.focus();
    expect(panel.style.paddingBottom).toBe("200px");
    resizePanel?.();
    expect(panel.getBoundingClientRect().height).toBe(200);

    viewport.height = 700;
    viewport.dispatchEvent(new Event("resize"));
    expect(panel.style.paddingBottom).toBe("100px");
  });

  it("recomputes a focused panel's inset when layout changes under an open keyboard", () => {
    input.focus();
    expect(panel.style.paddingBottom).toBe("300px");

    bottom = 500;
    resizePanel?.();
    expect(panel.style.paddingBottom).toBe("");

    bottom = 650;
    resizePanel?.();
    expect(panel.style.paddingBottom).toBe("150px");
  });

  it("follows layout changes while settling, then leaves an unfocused panel alone", () => {
    input.focus();
    input.blur();
    bottom = 550;
    resizePanel?.();
    expect(panel.style.paddingBottom).toBe("50px");

    bottom = 500;
    resizePanel?.();
    expect(panel.style.paddingBottom).toBe("");

    bottom = 800;
    resizePanel?.();
    viewport.dispatchEvent(new Event("resize"));
    expect(panel.style.paddingBottom).toBe("");
  });

  it("does not repeat padding writes for unchanged geometry", () => {
    const mutations = new MutationObserver(() => {});
    mutations.observe(panel, { attributes: true, attributeFilter: ["style"] });
    input.focus();
    expect(mutations.takeRecords()).toHaveLength(1);

    resizePanel?.();
    viewport.dispatchEvent(new Event("resize"));
    expect(mutations.takeRecords()).toHaveLength(0);

    bottom = 500;
    resizePanel?.();
    expect(mutations.takeRecords()).toHaveLength(1);
    resizePanel?.();
    viewport.dispatchEvent(new Event("resize"));
    expect(mutations.takeRecords()).toHaveLength(0);
    mutations.disconnect();
  });

  it("releases the geometry observer and input/viewport listeners on cleanup", () => {
    input.focus();
    cleanup?.();
    cleanup = undefined;
    expect(disconnect).toHaveBeenCalledOnce();
    expect(panel.style.paddingBottom).toBe("");

    input.blur();
    input.focus();
    viewport.dispatchEvent(new Event("resize"));
    expect(panel.style.paddingBottom).toBe("");
  });
});
