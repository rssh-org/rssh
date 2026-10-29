import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import TerminalKeybar from "./TerminalKeybar.svelte";
import { canHandleBack, registerBackHandler, requestBack } from "../stores/back.svelte.ts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn().mockResolvedValue(null) }));

let component: ReturnType<typeof mount> | undefined;
let removePage: (() => void) | undefined;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

async function render() {
  const target = document.createElement("div");
  document.body.append(target);
  component = mount(TerminalKeybar, { target });
  await settle();
  return target;
}

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  removePage?.();
  removePage = undefined;
  document.body.innerHTML = "";
});

describe("terminal extension keys system back", () => {
  it("dismisses the open extension panel before returning from the workspace", async () => {
    const pageBack = vi.fn();
    removePage = registerBackHandler({ handle: pageBack, priority: 0 });
    const target = await render();
    target.querySelector<HTMLButtonElement>('[aria-label="More keys"]')!.click();
    await settle();
    expect(target.querySelector(".ext-panel")).not.toBeNull();

    expect(requestBack()).toBe(true);
    await settle();
    expect(target.querySelector(".ext-panel")).toBeNull();
    expect(pageBack).not.toHaveBeenCalled();

    expect(requestBack()).toBe(true);
    expect(pageBack).toHaveBeenCalledOnce();
  });

  it("intercepts only while open and releases its handler when the bar unmounts", async () => {
    const target = await render();
    expect(canHandleBack()).toBe(false);
    const more = target.querySelector<HTMLButtonElement>('[aria-label="More keys"]')!;
    more.click();
    await settle();
    expect(canHandleBack()).toBe(true);

    target.querySelector<HTMLElement>(".ext-backdrop")!.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    await settle();
    expect(canHandleBack()).toBe(false);

    more.click();
    await settle();
    expect(canHandleBack()).toBe(true);
    await unmount(component!);
    component = undefined;
    await settle();
    expect(canHandleBack()).toBe(false);
  });
});
