import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount, type Component } from "svelte";
import { fromStore, writable } from "svelte/store";
import { on } from "svelte/events";
import ColorInput from "./ColorInput.svelte";
import GroupManager from "./GroupManager.svelte";
import HighlightRuleForm from "./HighlightRuleForm.svelte";
import { setLocale, t } from "../i18n/index.svelte.ts";

const { invokeMock, loadGroupsMock } = vi.hoisted(() => ({ invokeMock: vi.fn(), loadGroupsMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("../stores/app.svelte.ts", () => ({ loadGroups: loadGroupsMock }));

const mounted: Record<string, any>[] = [];

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

async function renderColor(initial: string, savedValue?: string) {
  const state = fromStore(writable(initial));
  const target = document.createElement("div");
  document.body.append(target);
  mounted.push(mount(ColorInput, {
    target,
    props: {
      get value() { return state.current; },
      set value(value: string) { state.current = value; },
      savedValue,
    },
  }));
  await settle();
  target.querySelector("details")!.open = true;
  return { target, state };
}

async function renderForm(component: Component<any>, props: Record<string, any> = {}) {
  const target = document.createElement("div");
  document.body.append(target);
  mounted.push(mount(component, { target, props }));
  await settle();
  return target;
}

function button(target: HTMLElement, text: string): HTMLButtonElement {
  const element = [...target.querySelectorAll("button")].find((candidate) => candidate.textContent?.trim() === text);
  expect(element, text).toBeDefined();
  return element!;
}

async function fill(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
}

function control(target: HTMLElement, part: string): HTMLElement {
  const element = target.querySelector<HTMLElement>(`[data-part="${part}"]`);
  expect(element, part).not.toBeNull();
  return element!;
}

async function key(element: HTMLElement, key: string, shiftKey = false) {
  element.focus();
  element.dispatchEvent(new KeyboardEvent("keydown", { key, shiftKey, bubbles: true, cancelable: true }));
  await settle();
}

function sizeArea(element: HTMLElement) {
  vi.spyOn(element, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 200, 100));
}

async function pointer(element: EventTarget, type: string, pointerType: string, x: number, y: number) {
  element.dispatchEvent(new PointerEvent(type, {
    pointerType, pointerId: pointerType === "touch" ? 2 : 1,
    clientX: x, clientY: y, button: 0, buttons: type === "pointerup" ? 0 : 1,
    bubbles: true, cancelable: true,
  }));
  await settle();
}

beforeEach(() => {
  setLocale("en");
  invokeMock.mockReset().mockResolvedValue(undefined);
  loadGroupsMock.mockReset().mockResolvedValue([]);
});

afterEach(async () => {
  for (const component of mounted.splice(0)) await unmount(component);
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("color picker form consumers", () => {
  it("saves a new group's chosen color, wave toggle and order while blocking incomplete HEX", async () => {
    const target = await renderForm(GroupManager);
    button(target, t("group.new")).click();
    await settle();
    await fill(target.querySelector<HTMLInputElement>('input[type="text"]')!, "New group");
    target.querySelector("details")!.open = true;
    const hex = target.querySelector<HTMLInputElement>('input[placeholder="#RRGGBB"]')!;
    await fill(hex, "#12");
    expect(button(target, t("common.save")).disabled).toBe(true);
    target.querySelector<HTMLButtonElement>('button[aria-label="#FF6B6B"]')!.click();
    await settle();
    const waveLabel = [...target.querySelectorAll("label")].find((label) => label.textContent === t("group.terminal_background"))!;
    waveLabel.click();
    await settle();
    expect(target.querySelector<HTMLInputElement>('input[role="switch"]')!.checked).toBe(true);
    await fill(target.querySelector<HTMLInputElement>('input[type="number"]')!, "7");
    button(target, t("common.save")).click();
    await settle();
    expect(invokeMock).toHaveBeenCalledWith("create_group", {
      group: expect.objectContaining({ name: "New group", color: "#FF6B6B", terminal_waves_enabled: true, sort_order: 7 }),
    });
  });

  it("saves an existing group's color, disabled wave toggle and changed order", async () => {
    const group = { id: "existing", name: "Production", color: "#FF0000", sort_order: 7, terminal_waves_enabled: true };
    loadGroupsMock.mockResolvedValue([group]);
    const target = await renderForm(GroupManager);
    target.querySelector<HTMLButtonElement>(`button[aria-label="${t("common.edit")} Production"]`)!.click();
    await settle();
    target.querySelector("details")!.open = true;
    await key(control(target, "channel-slider-thumb"), "ArrowRight");
    const selected = target.querySelector<HTMLInputElement>('input[placeholder="#RRGGBB"]')!.value;
    expect(selected).not.toBe(group.color);
    const waveToggle = target.querySelector<HTMLInputElement>('input[role="switch"]')!;
    expect(waveToggle.checked).toBe(true);
    expect(target.querySelector<HTMLInputElement>('input[type="number"]')!.value).toBe("7");
    waveToggle.click();
    await fill(target.querySelector<HTMLInputElement>('input[type="number"]')!, "2");
    button(target, t("common.save")).click();
    await settle();
    expect(invokeMock).toHaveBeenCalledWith("update_group", { group: { ...group, color: selected, terminal_waves_enabled: false, sort_order: 2 } });
  });

  it("preserves an imported highlight color on save and saves a new area selection", async () => {
    const rule = { keyword: "error", name: "Errors", color: "rgba(255, 0, 0, 0.5)", enabled: true, is_regex: true, is_case_sensitive: false };
    const onSave = vi.fn();
    const target = await renderForm(HighlightRuleForm, { rule, onSave, onCancel: vi.fn() });
    button(target, t("common.save")).click();
    expect(onSave).toHaveBeenLastCalledWith(rule);
    target.querySelector("details")!.open = true;
    await key(control(target, "area-thumb"), "ArrowLeft");
    const selected = target.querySelector<HTMLInputElement>('input[placeholder="#RRGGBB"]')!.value;
    expect(selected).toMatch(/^#[0-9A-F]{6}$/);
    button(target, t("common.save")).click();
    expect(onSave).toHaveBeenLastCalledWith({ ...rule, color: selected });
  });
});

describe("color picker integration", () => {
  it("updates the bound HEX color using the area and hue keyboard controls", async () => {
    const { target, state } = await renderColor("#FF0000");
    const area = control(target, "area-thumb");
    const hue = control(target, "channel-slider-thumb");
    expect(area.getAttribute("role")).toBe("slider");
    expect(hue.getAttribute("role")).toBe("slider");
    await key(area, "ArrowLeft");
    expect(state.current).toMatch(/^#[0-9A-F]{6}$/);
    expect(state.current).not.toBe("#FF0000");
    const afterArea = state.current;
    await key(hue, "ArrowRight");
    expect(state.current).not.toBe(afterArea);
    expect(target.querySelector<HTMLInputElement>('input[type="text"]')!.value).toBe(state.current);
  });

  it.each(["#000000", "#808080"])("retains the chosen hue at %s until saturation and brightness change", async (initial) => {
    const { target, state } = await renderColor(initial);
    const hue = control(target, "channel-slider-thumb");
    for (let i = 0; i < 12; i++) await key(hue, "ArrowRight", true);
    expect(hue.getAttribute("aria-valuenow")).toBe("120");
    expect(state.current).toBe(initial);
    const area = control(target, "area");
    sizeArea(area);
    await pointer(area, "pointerdown", "mouse", 200, 0);
    await pointer(document, "pointerup", "mouse", 200, 0);
    expect(state.current).toBe("#00FF00");
  });

  it("moves hue with PageUp and PageDown without resetting the selected color", async () => {
    const { target, state } = await renderColor("#00FF00");
    const hue = control(target, "channel-slider-thumb");
    await key(hue, "PageUp");
    expect(hue.getAttribute("aria-valuenow")).toBe("130");
    expect(state.current).toMatch(/^#[0-9A-F]{6}$/);
    await key(hue, "PageDown");
    expect(hue.getAttribute("aria-valuenow")).toBe("120");
    expect(state.current).toBe("#00FF00");
    await key(hue, "Home");
    await key(hue, "PageDown");
    expect(hue.getAttribute("aria-valuenow")).toBe("0");
    await key(hue, "End");
    await key(hue, "PageUp");
    expect(hue.getAttribute("aria-valuenow")).toBe("360");
    expect(state.current).toMatch(/^#[0-9A-F]{6}$/);
  });

  it("synchronizes parent drafts without rewriting valid shorthand or incomplete HEX", async () => {
    const { target, state } = await renderColor("#FF0000");
    const input = target.querySelector<HTMLInputElement>('input[type="text"]')!;
    state.current = "#0f0";
    await settle();
    expect(state.current).toBe("#0f0");
    expect(input.value).toBe("#0f0");
    expect(control(target, "channel-slider-thumb").getAttribute("aria-valuenow")).toBe("120");
    state.current = "#12";
    await settle();
    expect(state.current).toBe("#12");
    expect(input.value).toBe("#12");
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(document.getElementById(input.getAttribute("aria-describedby")!)).not.toBeNull();

    target.querySelector<HTMLButtonElement>('button[aria-label="#FF6B6B"]')!.click();
    await settle();
    expect(state.current).toBe("#FF6B6B");
    expect(input.value).toBe("#FF6B6B");
    expect(input.getAttribute("aria-invalid")).toBe("false");
  });

  it.each(["rgb(255, 0, 0)", "rgba(255, 0, 0, 0.5)", "#FF000080"])("does not overwrite the saved legacy color %s on initialization", async (saved) => {
    const { target, state } = await renderColor(saved, saved);
    expect(state.current).toBe(saved);
    const input = target.querySelector<HTMLInputElement>('input[type="text"]')!;
    expect(input.value).toBe(saved);
    expect(input.getAttribute("aria-invalid")).toBe("false");
  });

  it("preserves an unrecognized saved value without rendering it as a background image", async () => {
    const saved = 'url("https://example.invalid/color.png")';
    const { target, state } = await renderColor(saved, saved);
    expect(state.current).toBe(saved);
    expect(target.querySelector<HTMLElement>(".swatch")!.style.backgroundImage).toBe("");
  });

  it("keeps two mounted picker instances independent", async () => {
    const first = await renderColor("#FF0000");
    const second = await renderColor("#0000FF");
    const firstHue = control(first.target, "channel-slider-thumb");
    const secondHue = control(second.target, "channel-slider-thumb");
    expect(firstHue.id).not.toBe(secondHue.id);
    await key(firstHue, "ArrowRight");
    expect(first.state.current).not.toBe("#FF0000");
    expect(second.state.current).toBe("#0000FF");
    expect(secondHue.getAttribute("aria-valuenow")).toBe("240");
  });

  it("accepts touch dragging followed by a mouse gesture and clamps at the area edge", async () => {
    const { target, state } = await renderColor("#FF0000");
    const area = control(target, "area");
    sizeArea(area);
    await pointer(area, "pointerdown", "touch", 100, 0);
    expect(state.current).toBe("#FF8080");
    await pointer(document, "pointermove", "touch", 250, 150);
    expect(state.current).toBe("#000000");
    await pointer(document, "pointerup", "touch", 250, 150);
    await pointer(area, "pointerdown", "mouse", 200, 50);
    await pointer(document, "pointerup", "mouse", 200, 50);
    expect(state.current).toBe("#800000");
  });

  it("keeps color-area and hue touch gestures out of the app edge-swipe handler", async () => {
    const { target } = await renderColor("#FF0000");
    const onTouch = vi.fn();
    // Svelte's event helper preserves delegated-child stopPropagation ordering.
    const offStart = on(target, "touchstart", onTouch);
    const offEnd = on(target, "touchend", onTouch);
    for (const part of ["area", "channel-slider"]) {
      const element = control(target, part);
      element.dispatchEvent(new Event("touchstart", { bubbles: true }));
      element.dispatchEvent(new Event("touchend", { bubbles: true }));
    }
    expect(onTouch.mock.calls).toHaveLength(0);
    offStart();
    offEnd();
  });
});
