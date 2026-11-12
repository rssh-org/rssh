import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import ShellSettings from "./ShellSettings.svelte";
import * as app from "../stores/app.svelte.ts";
import { dismiss, toasts } from "../stores/toast.svelte.ts";
import { setLocale, t } from "../i18n/index.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

let component: ReturnType<typeof mount> | undefined;
let savePreference: ((value: string) => Promise<void>) | undefined;
let stored: string | null;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

async function renderSettings() {
  const target = document.createElement("div");
  document.body.append(target);
  component = mount(ShellSettings, { target });
  await settle();
  const toggle = target.querySelector<HTMLInputElement>("#split-opens-home")!;
  await vi.waitFor(() => {
    flushSync();
    expect(toggle.disabled).toBe(false);
  });
  return { target, toggle };
}

beforeEach(async () => {
  savePreference = undefined;
  stored = null;
  invokeMock.mockReset().mockImplementation(async (command, args) => {
    if (args?.key === "split_opens_home") {
      if (command === "set_setting") {
        await savePreference?.(args.value);
        stored = args.value;
      }
      return stored;
    }
    return command === "list_shells" ? [] : null;
  });
  await app.setSplitOpensHome(true);
  invokeMock.mockClear();
  setLocale("en");
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  for (const item of [...toasts()]) dismiss(item.id);
  document.body.innerHTML = "";
});

describe("split Home setting", () => {
  it("saves the clicked value and reflects the committed store state", async () => {
    const { target, toggle } = await renderSettings();
    expect(toggle.checked).toBe(true);
    expect(target.querySelector('label[for="split-opens-home"]')?.textContent)
      .toBe(t("settings.shell.split_opens_home"));
    expect(toggle.getAttribute("aria-describedby")).toBe("split-opens-home-hint");

    let completeSave!: () => void;
    savePreference = () => new Promise<void>((resolve) => { completeSave = resolve; });
    toggle.click();
    await settle();

    expect(toggle.disabled).toBe(true);
    expect(app.splitOpensHome()).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("set_setting", {
      key: "split_opens_home", value: "false",
    });
    completeSave();
    await vi.waitFor(() => {
      flushSync();
      expect(toggle.disabled).toBe(false);
    });
    expect(toggle.checked).toBe(false);
    expect(app.splitOpensHome()).toBe(false);
    expect(stored).toBe("false");

    savePreference = undefined;
    await app.setSplitOpensHome(true);
    await settle();
    expect(toggle.checked).toBe(true);
  });

  it("restores the checked state and reports a failed save", async () => {
    const { toggle } = await renderSettings();
    savePreference = async () => { throw new Error("settings unavailable"); };

    toggle.click();
    await vi.waitFor(() => {
      flushSync();
      expect(toasts()).toContainEqual(expect.objectContaining({
        kind: "error", message: "settings unavailable",
      }));
      expect(toggle.disabled).toBe(false);
    });

    expect(toggle.checked).toBe(true);
    expect(app.splitOpensHome()).toBe(true);
    expect(stored).toBe("true");
  });
});
