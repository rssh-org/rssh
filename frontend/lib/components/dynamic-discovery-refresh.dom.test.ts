import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import DynamicDiscoverySettings from "./DynamicDiscoverySettings.svelte";
import HomeScreen from "./HomeScreen.svelte";
import * as app from "../stores/app.svelte.ts";
import * as discovery from "../stores/discovery.svelte.ts";
import { setLocale, t } from "../i18n/index.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("../stores/runtime.svelte.ts", () => ({
  capabilities: () => ({ localDiscovery: true, localPty: true, serial: false }),
}));

const components: ReturnType<typeof mount>[] = [];
const target = {
  id: "docker:example", source_id: "docker", source_name: "Development",
  platform: "docker", name: "refreshed-container", sub: "running",
  connector_spec: { type: "docker_exec", context: "default", container_id: "example", container_name: "refreshed-container", shell: "sh" },
};

async function settle() {
  for (let i = 0; i < 4; i++) { flushSync(); await tick(); }
}

function render(component: typeof DynamicDiscoverySettings | typeof HomeScreen) {
  const host = document.createElement("div");
  document.body.append(host);
  components.push(mount(component, { target: host }));
  return host;
}

beforeEach(async () => {
  setLocale("en");
  app.closeSettings();
  app.setActiveWorkspace("home");
  invokeMock.mockReset().mockImplementation(async (command) => {
    if (command === "get_setting") return JSON.stringify({ auto_refresh: false, interval_seconds: 60 });
    if (command === "dynamic_discovery_tool_status") return { available: true, version: "1", error: null };
    if (command === "discover_dynamic_targets") return { targets: [], errors: [] };
    return [];
  });
  await discovery.loadPreferences();
  await discovery.refresh();
  invokeMock.mockClear();
});

afterEach(async () => {
  for (const component of components.splice(0)) await unmount(component);
  discovery.stopBackgroundRefresh();
  app.closeSettings();
  document.body.innerHTML = "";
});

describe("shared discovery refresh controls", () => {
  it("places manual and automatic controls below New source and publishes manual results to Home", async () => {
    const home = render(HomeScreen);
    const settings = render(DynamicDiscoverySettings);
    await settle();
    expect(invokeMock.mock.calls.some(([command]) => command === "discover_dynamic_targets")).toBe(false);
    const block = settings.querySelector<HTMLElement>('[aria-labelledby="discovery-refresh-title"]')!;
    expect(block).not.toBeNull();
    expect(settings.querySelector(".toolbar")!.compareDocumentPosition(block) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(block.querySelector('input[type="checkbox"]')).not.toBeNull();
    expect(block.querySelector('input[type="number"]')).not.toBeNull();
    const refresh = [...block.querySelectorAll("button")].find((button) => button.textContent === t("dynamic_discovery.manual_refresh"))!;
    expect(refresh).toBeDefined();
    invokeMock.mockImplementation(async (command) => command === "discover_dynamic_targets" ? { targets: [target], errors: [] } : []);
    refresh.click();
    await settle();
    expect(home.textContent).toContain("refreshed-container");
    app.openSettings();
    await settle();
    app.closeSettings();
    await settle();
    expect(invokeMock.mock.calls.filter(([command]) => command === "discover_dynamic_targets")).toHaveLength(1);
  });

  it("persists the shared interval without running discovery when automatic refresh is disabled", async () => {
    const settings = render(DynamicDiscoverySettings);
    await settle();
    const interval = settings.querySelector<HTMLInputElement>('input[type="number"]')!;
    expect(interval).not.toBeNull();
    interval.value = "120";
    interval.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(invokeMock).toHaveBeenCalledWith("set_setting", {
      key: "dynamic_discovery_refresh", value: JSON.stringify({ auto_refresh: false, interval_seconds: 120 }),
    });
    expect(invokeMock.mock.calls.some(([command]) => command === "discover_dynamic_targets")).toBe(false);
  });

  it("restores the switch when saving fails so its display agrees with the active schedule", async () => {
    const settings = render(DynamicDiscoverySettings);
    await settle();
    const toggle = settings.querySelector<HTMLInputElement>('.refresh-settings input[type="checkbox"]')!;
    expect(toggle.checked).toBe(false);
    invokeMock.mockRejectedValueOnce(new Error("settings read-only"));
    toggle.click();
    await settle();
    expect(toggle.checked).toBe(false);
    expect(discovery.preferences().auto_refresh).toBe(false);
  });

  it.each([false, true])("invalidates Home targets only after a source disable is saved (save fails=%s)", async (fails) => {
    let sources = [{ id: "docker", name: "Development", enabled: true, platform: "docker", context: "default", shell: "sh" }];
    invokeMock.mockImplementation(async (command, args) => {
      if (command === "list_dynamic_discovery_sources") return sources;
      if (command === "dynamic_discovery_tool_status") return { available: true, version: "1", error: null };
      if (command === "discover_dynamic_targets") return { targets: [target], errors: [] };
      if (command === "save_dynamic_discovery_sources") {
        if (fails) throw new Error("settings read-only");
        sources = args.sources;
      }
      return [];
    });
    await discovery.refresh();
    invokeMock.mockClear();
    const home = render(HomeScreen);
    const settings = render(DynamicDiscoverySettings);
    await settle();
    expect(home.textContent).toContain(target.name);
    const toggle = settings.querySelector<HTMLInputElement>('.item-actions input[type="checkbox"]')!;
    toggle.click();
    await settle();
    expect(toggle.checked).toBe(fails);
    expect(home.textContent!.includes(target.name)).toBe(fails);
    expect(invokeMock.mock.calls.some(([command]) => command === "discover_dynamic_targets")).toBe(false);
  });

  it("blocks all source edits during a pending save and preserves both sequential disables", async () => {
    let sources = ["first", "second"].map((id) => ({
      id, name: id, enabled: true, platform: "docker", context: "default", shell: "sh",
    }));
    let finishSave!: () => void;
    invokeMock.mockImplementation(async (command, args) => {
      if (command === "list_dynamic_discovery_sources") return sources;
      if (command === "dynamic_discovery_tool_status") return { available: true, version: "1", error: null };
      if (command === "save_dynamic_discovery_sources") {
        await new Promise<void>((resolve) => { finishSave = resolve; });
        sources = args.sources;
      }
      return [];
    });
    const settings = render(DynamicDiscoverySettings);
    await settle();
    const toggles = () => [...settings.querySelectorAll<HTMLInputElement>('.item-actions input[type="checkbox"]')];
    toggles()[0].click();
    await settle();
    expect(toggles().every((input) => input.matches(":disabled"))).toBe(true);
    expect([...settings.querySelectorAll<HTMLButtonElement>(".toolbar button, .item-actions button")]
      .every((button) => button.matches(":disabled"))).toBe(true);
    toggles()[1].click();
    await settle();
    expect(invokeMock.mock.calls.filter(([command]) => command === "save_dynamic_discovery_sources")).toHaveLength(1);

    finishSave();
    await vi.waitFor(() => expect(toggles().every((input) => !input.matches(":disabled"))).toBe(true));
    expect(toggles().map((input) => input.checked)).toEqual([false, true]);
    toggles()[1].click();
    await settle();
    finishSave();
    await vi.waitFor(() => expect(toggles().every((input) => !input.matches(":disabled"))).toBe(true));
    expect(sources.map((source) => source.enabled)).toEqual([false, false]);
    expect(toggles().map((input) => input.checked)).toEqual([false, false]);
  });
});
