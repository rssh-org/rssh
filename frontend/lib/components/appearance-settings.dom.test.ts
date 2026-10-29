import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import AppearanceSettings from "./AppearanceSettings.svelte";
import * as ai from "../ai/store.svelte.ts";
import * as layout from "../stores/layout.svelte.ts";
import * as runtime from "../stores/runtime.svelte.ts";
import { setLocale, t } from "../i18n/index.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

let component: ReturnType<typeof mount> | undefined;
let stopViewport: (() => void) | undefined;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

async function resize(width: number) {
  window.innerWidth = width;
  window.dispatchEvent(new Event("resize"));
  await settle();
}

beforeEach(async () => {
  invokeMock.mockReset().mockImplementation(async (command: string) => {
    if (command === "get_runtime_capabilities") return {
      ...runtime.capabilities(),
      terminalPolicy: {
        imageStorageLimitMb: 128,
        imagePixelLimit: 16_000_000,
        outputBacklogLimitBytes: 8_000_000,
        gpuRenderDefault: false,
      },
    };
    return command === "list_fonts" ? [] : null;
  });
  await runtime.load();
  setLocale("en");
  ai.setPosition("right");
  window.innerWidth = 640;
  stopViewport = layout.observeViewport();
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  stopViewport?.();
  stopViewport = undefined;
  ai.setPosition("right");
  document.body.innerHTML = "";
});

describe("appearance AI panel position", () => {
  it("hides the complete position setting below 640px and preserves the selected wide position", async () => {
    const target = document.createElement("div");
    document.body.append(target);
    component = mount(AppearanceSettings, { target });
    await settle();

    const positionLabel = () => [...target.querySelectorAll(".section-label")]
      .find((label) => label.textContent === t("settings.appearance.ai_panel_position"));
    const positionButtons = () => positionLabel()?.nextElementSibling?.querySelectorAll<HTMLButtonElement>("button");

    expect(positionButtons()).toHaveLength(2);
    positionButtons()![0].click();
    await settle();
    expect(ai.position()).toBe("left");
    expect(localStorage.getItem("ai_panel_position")).toBe("left");

    await resize(639);
    expect(positionLabel()).toBeUndefined();
    expect(target.querySelectorAll(".mini-ai")).toHaveLength(0);
    expect(target.textContent).toContain(t("settings.appearance.sidebar_position"));
    expect(ai.position()).toBe("left");
    expect(localStorage.getItem("ai_panel_position")).toBe("left");

    await resize(640);
    expect(positionButtons()).toHaveLength(2);
    expect(positionButtons()![0].classList.contains("active")).toBe(true);
    expect(positionButtons()![1].classList.contains("active")).toBe(false);
    expect(ai.position()).toBe("left");
    expect(localStorage.getItem("ai_panel_position")).toBe("left");
  });
});
