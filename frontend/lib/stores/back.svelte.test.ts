import { beforeEach, describe, expect, it, vi } from "vitest";

beforeEach(() => vi.resetModules());

describe("application back navigation", () => {
  it("dismisses the topmost layer without also navigating the page underneath", async () => {
    const back = await import("./back.svelte.ts");
    const page = vi.fn();
    const dialog = vi.fn();
    back.registerBackHandler({ handle: page, priority: 0 });
    const removeDialog = back.registerBackHandler({ handle: dialog });

    expect(back.requestBack()).toBe(true);
    expect(dialog).toHaveBeenCalledOnce();
    expect(page).not.toHaveBeenCalled();
    removeDialog();
    expect(back.requestBack()).toBe(true);
    expect(page).toHaveBeenCalledOnce();
  });

  it("lets the host handle back when the page has no destination", async () => {
    const back = await import("./back.svelte.ts");
    let open = true;
    const handle = vi.fn(() => { open = false; });
    back.registerBackHandler({ enabled: () => open, handle, priority: 0 });
    expect(back.canHandleBack()).toBe(true);
    expect(back.requestBack()).toBe(true);
    expect(back.canHandleBack()).toBe(false);
    expect(back.requestBack()).toBe(false);
    expect(handle).toHaveBeenCalledOnce();
  });

  it("keeps a newer overlay registered when an older component is destroyed", async () => {
    const back = await import("./back.svelte.ts");
    const old = back.registerBackHandler({ handle: vi.fn() });
    const current = vi.fn();
    const removeCurrent = back.registerBackHandler({ handle: current });
    old();
    old();
    expect(back.requestBack()).toBe(true);
    expect(current).toHaveBeenCalledOnce();
    removeCurrent();
    expect(back.canHandleBack()).toBe(false);
  });
});
