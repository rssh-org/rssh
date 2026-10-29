import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import SettingsLayout from "./SettingsLayout.svelte";
import * as app from "../stores/app.svelte.ts";
import * as layout from "../stores/layout.svelte.ts";
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

async function renderSettings() {
  const target = document.createElement("div");
  document.body.append(target);
  component = mount(SettingsLayout, { target });
  await settle();
  return target;
}

beforeEach(() => {
  invokeMock.mockReset().mockResolvedValue([]);
  app.closeSettings();
  app.settingsNavigate("menu");
  setLocale("en");
  window.innerWidth = 400;
  stopViewport = layout.observeViewport();
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  stopViewport?.();
  stopViewport = undefined;
  document.body.innerHTML = "";
});

describe("settings page header", () => {
  it("keeps the same unsaved form while moving between compact and wide layouts", async () => {
    app.navigate("credential-edit");
    const target = await renderSettings();
    const name = target.querySelector<HTMLInputElement>('input[placeholder="prod-key"]')!;
    const secret = target.querySelector<HTMLInputElement>('input[type="password"]')!;
    name.value = "Unsaved credential";
    name.dispatchEvent(new Event("input", { bubbles: true }));
    secret.value = "unsaved-secret";
    secret.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    expect(target.querySelector("h1")?.textContent).toBe(t("settings.credential_create"));
    expect(target.querySelector("nav")).toBeNull();

    await resize(900);
    expect(target.querySelector("h1")).toBeNull();
    expect(target.querySelector("nav")).not.toBeNull();
    expect(target.querySelector('input[placeholder="prod-key"]')).toBe(name);
    expect(target.querySelector('input[type="password"]')).toBe(secret);
    expect(name.value).toBe("Unsaved credential");
    expect(secret.value).toBe("unsaved-secret");

    await resize(400);
    expect(target.querySelector("h1")?.textContent).toBe(t("settings.credential_create"));
    expect(target.querySelector("nav")).toBeNull();
    expect(target.querySelector('input[placeholder="prod-key"]')).toBe(name);
    expect(target.querySelector('input[type="password"]')).toBe(secret);
    expect(name.value).toBe("Unsaved credential");
    expect(secret.value).toBe("unsaved-secret");
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("uses one compact title and returns from an editor through its parent to the workspace", async () => {
    app.navigate("credential-edit");
    const target = await renderSettings();
    const back = () => target.querySelector<HTMLButtonElement>('header button[aria-label="Back"]')!;

    back().click();
    await settle();
    expect(app.settingsPage()).toBe("credentials");
    expect(target.querySelector("h1")?.textContent).toBe(t("settings.section.credentials"));

    back().click();
    await settle();
    expect(app.settingsPage()).toBe("menu");
    expect(target.querySelectorAll("h1")).toHaveLength(1);
    expect(target.querySelector("h1")?.textContent).toBe(t("settings.title"));
    expect(target.querySelector(".menu-header")).toBeNull();
    expect(target.querySelector("nav")).not.toBeNull();

    back().click();
    await settle();
    expect(app.settingsActive()).toBe(false);
    expect(app.activeWorkspaceId()).toBe("home");
  });

  it("updates the header and back label when the language changes", async () => {
    app.openSettings();
    const target = await renderSettings();

    setLocale("zh");
    await settle();

    expect(target.querySelector("h1")?.textContent).toBe("设置");
    expect(target.querySelector('header button[aria-label="返回"]')).not.toBeNull();
  });

  it("uses the same back dispatch for the header and an open page dropdown", async () => {
    app.openSettings();
    const target = await renderSettings();
    const dropdown = target.querySelector<HTMLButtonElement>('[aria-label="Language"]')!;
    const back = target.querySelector<HTMLButtonElement>('header button[aria-label="Back"]')!;
    dropdown.click();
    await settle();
    expect(dropdown.getAttribute("aria-expanded")).toBe("true");

    back.click();
    await settle();
    expect(dropdown.getAttribute("aria-expanded")).toBe("false");
    expect(app.settingsActive()).toBe(true);

    back.click();
    await settle();
    expect(app.settingsActive()).toBe(false);
  });
});
