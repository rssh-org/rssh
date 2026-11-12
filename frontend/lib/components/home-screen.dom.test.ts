import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import HomeScreen from "./HomeScreen.svelte";
import { setLocale } from "../i18n/index.svelte.ts";
import { registerBackHandler } from "../stores/back.svelte.ts";

const app = vi.hoisted(() => ({
  activePane: "home:a",
  workspace: "ssh:root",
  settings: false,
  localPty: true,
  openTab: vi.fn(),
  loadProfiles: vi.fn(),
  setActivePane: vi.fn(),
}));

vi.mock("../stores/app.svelte.ts", () => ({
  tabs: () => [
    { id: "home", type: "home" },
    { id: "home:a", type: "home", workspaceId: "ssh:root", paneOf: "ssh:root" },
    { id: "home:b", type: "home", workspaceId: "ssh:root", paneOf: "ssh:root" },
  ],
  activeTabId: () => app.activePane,
  activeWorkspaceId: () => app.workspace,
  settingsActive: () => app.settings,
  capabilities: () => ({ localPty: app.localPty, serial: false, localDiscovery: false }),
  loadProfiles: app.loadProfiles,
  loadCredentials: async () => [],
  loadForwards: async () => [],
  loadGroups: async () => [],
  loadTelnetProfiles: async () => [],
  recentHomeItemIds: () => [],
  isProfilePinned: () => false,
  openTab: app.openTab,
  setActivePane: app.setActivePane,
}));
vi.mock("../stores/sync.svelte.ts", () => ({ configurationRevision: () => 0 }));

const components: ReturnType<typeof mount>[] = [];

async function settle() {
  flushSync();
  await tick();
  await Promise.resolve();
  flushSync();
}

async function renderHome(paneId = "home") {
  const target = document.createElement("div");
  target.className = "pane";
  document.body.append(target);
  components.push(mount(HomeScreen, { target, props: { paneId } }));
  await settle();
  await settle();
  return target;
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  setLocale("en");
  app.activePane = "home:a";
  app.workspace = "ssh:root";
  app.settings = false;
  app.localPty = true;
  app.setActivePane.mockImplementation((paneId: string) => { app.activePane = paneId; });
  app.loadProfiles.mockResolvedValue([
    { id: "profile:one", name: "Production", host: "example.com", port: 22, credential_id: "credential:one", group_id: null },
  ]);
});

afterEach(async () => {
  for (const component of components.splice(0)) await unmount(component);
  document.body.innerHTML = "";
});

describe("Home in split panes", () => {
  it.each(["home", "home:a"])("navigates %s from body focus and yields to transient UI", async (paneId) => {
    app.workspace = "home";
    app.activePane = "home";
    await renderHome();
    app.workspace = "ssh:root";
    await renderHome("home:a");
    await renderHome("home:b");
    app.workspace = paneId === "home" ? "home" : "ssh:root";
    app.activePane = paneId;

    const arrow = new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true });
    document.body.dispatchEvent(arrow);
    await settle();
    expect(arrow.defaultPrevented).toBe(true);
    expect(document.querySelectorAll(".card-btn.selected")).toHaveLength(1);

    const unregister = registerBackHandler({ handle: vi.fn() });
    try {
      for (const key of ["ArrowDown", "Enter"]) {
        const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
        document.body.dispatchEvent(event);
        expect(event.defaultPrevented).toBe(false);
        expect(app.openTab).not.toHaveBeenCalled();
      }
    } finally {
      unregister();
    }

    const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    document.body.dispatchEvent(enter);
    expect(enter.defaultPrevented).toBe(true);
    expect(app.openTab).toHaveBeenCalledTimes(1);
    expect(app.openTab).toHaveBeenCalledWith(expect.objectContaining({ type: "ssh", label: "Production" }), paneId);
  });

  it("loads visible Home panes and sends keyboard selection only to the focused pane", async () => {
    await renderHome();
    const first = await renderHome("home:a");
    const second = await renderHome("home:b");
    expect(app.loadProfiles).toHaveBeenCalledTimes(2);
    expect(first.querySelector(".card-name")?.textContent).toBe("Production");
    expect(second.querySelector(".card-name")?.textContent).toBe("Production");
    expect(first.querySelector("h2")?.id).not.toBe(second.querySelector("h2")?.id);

    first.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await settle();
    first.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(app.openTab).toHaveBeenCalledTimes(1);
    expect(app.openTab).toHaveBeenCalledWith(expect.objectContaining({ type: "ssh", label: "Production" }), "home:a");

    app.openTab.mockClear();
    app.activePane = "ssh:root";
    first.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(app.openTab).not.toHaveBeenCalled();
  });

  it("focuses the correct Home without intercepting typing and opens local terminals in that pane", async () => {
    const first = await renderHome("home:a");
    const second = await renderHome("home:b");
    const search = second.querySelector<HTMLInputElement>(".search-input")!;
    search.focus();
    expect(app.activePane).toBe("home:b");
    search.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    search.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(app.openTab).not.toHaveBeenCalled();

    second.querySelector<HTMLButtonElement>(".home-controls > .btn")!.click();
    expect(app.openTab).toHaveBeenCalledWith(expect.objectContaining({ type: "local" }), "home:b");
    app.openTab.mockClear();
    first.querySelector<HTMLButtonElement>(".card-btn")!.click();
    expect(app.openTab).toHaveBeenCalledWith(expect.objectContaining({ type: "ssh" }), "home:a");
  });

  it("leaves external buttons and Home actions usable before and after grid selection", async () => {
    const pane = await renderHome("home:a");
    const externalButton = document.createElement("button");
    externalButton.textContent = "Confirm close";
    document.body.append(externalButton);
    const localButton = pane.querySelector<HTMLButtonElement>(".home-controls > .btn")!;
    // Action buttons must also work when moved out of the controls container.
    pane.querySelector(".home")!.append(localButton);

    for (const selected of [false, true]) {
      if (selected) {
        pane.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
        await settle();
        expect(pane.querySelector(".card-btn.selected")).not.toBeNull();
      }
      for (const button of [externalButton, localButton]) {
        const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
        button.dispatchEvent(enter);
        expect(enter.defaultPrevented).toBe(false);
        expect(app.openTab).not.toHaveBeenCalled();
      }
    }
  });

  it("respects keyboard events already handled by a child control", async () => {
    const pane = await renderHome("home:a");
    const card = pane.querySelector<HTMLButtonElement>(".card-btn")!;
    const handled = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    handled.preventDefault();
    card.dispatchEvent(handled);
    await settle();
    expect(pane.querySelector(".card-btn.selected")).toBeNull();
    expect(app.openTab).not.toHaveBeenCalled();

    card.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    await settle();
    card.dispatchEvent(handled);
    expect(app.openTab).not.toHaveBeenCalled();
  });

  it("keeps the primary Home behavior and respects local PTY capability", async () => {
    app.workspace = "home";
    app.activePane = "home";
    const primary = await renderHome();
    expect(primary.querySelector(".home-controls > .btn")).toBeNull();
    primary.querySelector<HTMLButtonElement>(".card-btn")!.click();
    expect(app.openTab).toHaveBeenCalledWith(expect.objectContaining({ type: "ssh" }), "home");

    app.workspace = "ssh:root";
    app.localPty = false;
    const pane = await renderHome("home:a");
    expect(pane.querySelector(".home-controls > .btn")).toBeNull();
  });
});
