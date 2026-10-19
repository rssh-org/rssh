import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const key = "session_restore.desktop";
let settings: Map<string, string>;

beforeEach(() => {
  vi.resetModules();
  settings = new Map();
  vi.stubGlobal("navigator", { userAgent: "node" });
  const storage = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => storage.set(key, value),
    removeItem: (key: string) => storage.delete(key),
  });
  invoke.mockReset();
  invoke.mockImplementation(async (command: string, args: Record<string, string>) => {
    if (command === "get_setting") return settings.get(args.key) ?? null;
    if (command === "set_setting") { settings.set(args.key, args.value); return; }
    if (command === "list_profiles") return [{ id: "ssh1", name: "Current SSH", host: "new-host", port: 2222 }];
    if (command === "list_telnet_profiles") return [{ id: "telnet1", name: "Telnet", host: "telnet", port: 23, login_script: "secret" }];
    if (command === "list_serial_profiles") return [];
    return null;
  });
});
afterEach(() => vi.unstubAllGlobals());

async function modules() {
  return { app: await import("./app.svelte.ts"), restore: await import("./session-restore.svelte.ts") };
}
function saved(terminals: unknown[], active = -1) {
  settings.set(`${key}.enabled`, "true");
  settings.set(`${key}.snapshot`, JSON.stringify({ version: 1, terminals, active }));
}

describe("session restoration", () => {
  it("defaults off without fetching profiles or reopening tabs", async () => {
    const { app, restore } = await modules();
    expect(await restore.restore()).toBe(false);
    expect(restore.enabled()).toBe(false);
    expect(restore.ready()).toBe(true);
    expect(app.tabs()).toHaveLength(1);
    expect(invoke).not.toHaveBeenCalledWith("list_profiles");
  });

  it("reopens duplicates in order with fresh IDs and current profiles, skipping deleted profiles", async () => {
    saved([
      { type: "ssh", profileId: "ssh1" }, { type: "ssh", profileId: "gone" },
      { type: "local" }, { type: "ssh", profileId: "ssh1" },
      { type: "telnet", profileId: "telnet1" },
    ], 3);
    const { app, restore } = await modules();
    expect(await restore.restore()).toBe(true);
    expect(app.tabs().map((tab) => tab.type)).toEqual(["home", "ssh", "local", "ssh", "telnet"]);
    expect(new Set(app.tabs().map((tab) => tab.id)).size).toBe(5);
    expect(app.activeTabId()).toBe(app.tabs()[3].id);
    expect(app.tabs()[1].meta).toEqual({ profileId: "ssh1", host: "new-host", port: "2222" });
    expect(app.tabs()[4].meta).not.toHaveProperty("login_script");
  });

  it("persists only profile references, including split panes, without secret metadata or unsupported tabs", async () => {
    const { app, restore } = await modules();
    app.addTab({ id: "ssh", type: "ssh", label: "Secret label", meta: { profileId: "ssh1", password: "secret", host: "host" } });
    app.addPane("ssh", "right", { id: "telnet", type: "telnet", label: "Telnet", meta: { profileId: "telnet1", login_script: "secret" } });
    app.addTab({ id: "serial", type: "serial", label: "Serial", meta: { serialProfileId: "serial1", login_script: "secret" } });
    app.addTab({ id: "forward", type: "forward", label: "Forward", meta: { forwardId: "f1" } });
    app.addTab({ id: "container", type: "docker_exec", label: "Container", meta: { connectorSpec: "secret" } });
    app.addTab({ id: "editor", type: "edit", label: "Editor", meta: { content: "secret" } });
    app.setActiveWorkspace("ssh");
    app.setActivePane("telnet");
    const snapshot = restore.snapshot();
    expect(JSON.parse(snapshot)).toEqual({ version: 1, terminals: [
      { type: "ssh", profileId: "ssh1" }, { type: "telnet", profileId: "telnet1" },
      { type: "serial", profileId: "serial1" },
    ], active: 1 });
    expect(snapshot).not.toContain("secret");
  });

  it("clears saved sessions when disabled and does not persist while disabled", async () => {
    saved([{ type: "local" }]);
    const { restore } = await modules();
    await restore.restore();
    await restore.setEnabled(false);
    restore.persist(restore.snapshot());
    expect(settings.get(`${key}.enabled`)).toBe("false");
    expect(settings.get(`${key}.snapshot`)).toBe("");
  });

  it.each(["bad JSON", '{"version":2,"terminals":[]}', '{"version":1,"terminals":[{"type":"ssh"}]}'])
    ("handles invalid snapshots: %s", async (snapshot) => {
      settings.set(`${key}.enabled`, "true");
      settings.set(`${key}.snapshot`, snapshot);
      const { app, restore } = await modules();
      expect(await restore.restore()).toBe(false);
      expect(app.tabs()).toHaveLength(1);
    });

  it("does not overwrite the snapshot if profile loading fails", async () => {
    saved([{ type: "ssh", profileId: "ssh1" }]);
    const original = settings.get(`${key}.snapshot`);
    const implementation = invoke.getMockImplementation()!;
    invoke.mockImplementation((command, args) => command === "list_profiles"
      ? Promise.reject(new Error("offline")) : implementation(command, args));
    const { restore } = await modules();
    await restore.restore();
    restore.persist(restore.snapshot());
    expect(settings.get(`${key}.snapshot`)).toBe(original);
  });

  it("does not reopen connections after startup cancellation", async () => {
    saved([{ type: "local" }]);
    const controller = new AbortController();
    controller.abort();
    const { app, restore } = await modules();
    expect(await restore.restore(controller.signal)).toBe(false);
    expect(app.tabs()).toHaveLength(1);
    expect(restore.ready()).toBe(false);
  });

  it("does not take over connections opened manually during startup", async () => {
    saved([{ type: "local" }]);
    const { app, restore } = await modules();
    app.addTab({ id: "manual", type: "local", label: "Manual" });
    expect(await restore.restore()).toBe(false);
    expect(app.tabs().map((tab) => tab.id)).toEqual(["home", "manual"]);
  });

  it("serializes snapshots so the latest write wins", async () => {
    const { restore } = await modules();
    await restore.restore();
    await restore.setEnabled(true);
    restore.persist("first");
    restore.persist("second");
    await vi.waitFor(() => expect(settings.get(`${key}.snapshot`)).toBe("second"));
  });

  it("keeps saved ordering even when MRU reordering is enabled", async () => {
    saved([{ type: "ssh", profileId: "ssh1" }, { type: "local" }], 1);
    const { app, restore } = await modules();
    await app.setTabMru(true);
    await restore.restore();
    expect(app.tabs().map((tab) => tab.type)).toEqual(["home", "ssh", "local"]);
    expect(app.activeTabId()).toBe(app.tabs()[2].id);
  });

  it("restores mobile SSH/Telnet while excluding desktop local and serial terminals", async () => {
    vi.stubGlobal("navigator", { userAgent: "Android" });
    settings.set("session_restore.mobile.enabled", "true");
    settings.set("session_restore.mobile.snapshot", JSON.stringify({ version: 1, terminals: [
      { type: "local" }, { type: "serial", profileId: "s1" },
      { type: "ssh", profileId: "ssh1" }, { type: "telnet", profileId: "telnet1" },
    ], active: 2 }));
    const { app, restore } = await modules();
    expect(await restore.restore()).toBe(false);
    expect(app.tabs().map((tab) => tab.type)).toEqual(["home", "ssh", "telnet"]);
    expect(invoke).not.toHaveBeenCalledWith("list_serial_profiles");
    expect(app.activeTabId()).toBe(app.tabs()[1].id);
  });
});
