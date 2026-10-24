import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => { vi.unstubAllGlobals(); });

describe("viewport layout", () => {
  it("uses a stable wide layout in hosts without viewport dimensions", async () => {
    vi.stubGlobal("window", new EventTarget());
    vi.resetModules();
    const layout = await import("./layout.svelte.ts");
    expect(layout.viewportWidth()).toBe(1024);
    expect(layout.compact()).toBe(false);
  });

  it("switches at 640 CSS pixels and stops observing when the root unmounts", async () => {
    const viewport = Object.assign(new EventTarget(), { innerWidth: 640 });
    vi.stubGlobal("window", viewport);
    vi.resetModules();
    const layout = await import("./layout.svelte.ts");
    const stop = layout.observeViewport();

    expect(layout.compact()).toBe(false);
    viewport.innerWidth = 639;
    viewport.dispatchEvent(new Event("resize"));
    expect(layout.viewportWidth()).toBe(639);
    expect(layout.compact()).toBe(true);

    viewport.innerWidth = 900;
    viewport.dispatchEvent(new Event("resize"));
    expect(layout.compact()).toBe(false);

    stop();
    viewport.innerWidth = 320;
    viewport.dispatchEvent(new Event("resize"));
    expect(layout.viewportWidth()).toBe(900);
  });

  it("reads the latest width when observation starts after module loading", async () => {
    const viewport = Object.assign(new EventTarget(), { innerWidth: 900 });
    vi.stubGlobal("window", viewport);
    vi.resetModules();
    const layout = await import("./layout.svelte.ts");

    viewport.innerWidth = 400;
    const stop = layout.observeViewport();
    expect(layout.viewportWidth()).toBe(400);
    expect(layout.compact()).toBe(true);
    stop();
  });
});
