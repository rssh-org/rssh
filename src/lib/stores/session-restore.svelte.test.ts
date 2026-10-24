import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
const capabilities = vi.hoisted(() => ({ localPty: true, serial: true }));
vi.mock("./runtime.svelte.ts", () => ({ capabilities: () => capabilities }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const key = "session_restore";
let settings: Map<string, string>;

beforeEach(() => {
  vi.resetModules();
  settings = new Map();
  capabilities.localPty = true;
  capabilities.serial = true;
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
function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("session restoration", () => {
  it("defaults off without fetching profiles or reopening tabs", async () => {
    const { app, restore } = await modules();
    expect(await restore.restore()).toBe(false);
    expect(await restore.enabled()).toBe(false);
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
    await restore.persist(restore.snapshot());
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
    expect(restore.ready()).toBe(false);
    await restore.persist(restore.snapshot());
    expect(settings.get(`${key}.snapshot`)).toBe(original);
  });

  it("does not reopen connections after startup cancellation", async () => {
    saved([{ type: "local" }]);
    const original = settings.get(`${key}.snapshot`);
    const controller = new AbortController();
    controller.abort();
    const { app, restore } = await modules();
    expect(await restore.restore(controller.signal)).toBe(false);
    expect(app.tabs()).toHaveLength(1);
    expect(restore.ready()).toBe(false);
    await restore.persist(restore.snapshot());
    expect(settings.get(`${key}.snapshot`)).toBe(original);
  });

  it("does not reopen saved connections when disabled during profile loading", async () => {
    saved([{ type: "ssh", profileId: "ssh1" }]);
    const { app, restore } = await modules();
    const blocked = deferred();
    const implementation = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (command, args) => {
      if (command === "list_profiles") await blocked.promise;
      return implementation(command, args);
    });
    const startup = restore.restore();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("list_profiles"));
    await restore.setEnabled(false);
    blocked.resolve();
    expect(await startup).toBe(false);
    expect(app.tabs().map((tab) => tab.id)).toEqual(["home"]);
    expect(settings.get(`${key}.snapshot`)).toBe("");
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
    await Promise.all([restore.persist("first"), restore.persist("second")]);
    expect(settings.get(`${key}.snapshot`)).toBe("second");
  });

  it("reads the current enabled preference instead of caching it", async () => {
    const { restore } = await modules();
    expect(await restore.enabled()).toBe(false);
    settings.set(`${key}.enabled`, "true");
    expect(await restore.enabled()).toBe(true);
    settings.set(`${key}.enabled`, "false");
    expect(await restore.enabled()).toBe(false);
  });

  it("observes preference changes from a secondary window before saving primary tabs", async () => {
    const primary = await modules();
    await primary.restore.restore();
    primary.app.addTab({ id: "primary", type: "local", label: "Primary" });
    vi.resetModules();
    const secondary = await modules();
    secondary.app.addTab({ id: "secondary", type: "local", label: "Secondary" });

    await secondary.restore.setEnabled(true);
    expect(secondary.restore.ready()).toBe(false);
    await secondary.restore.persist(secondary.restore.snapshot());
    expect(settings.get(`${key}.snapshot`)).toBeUndefined();
    await primary.restore.persist(primary.restore.snapshot());
    expect(settings.get(`${key}.snapshot`)).toBe(primary.restore.snapshot());

    await secondary.restore.setEnabled(false);
    await primary.restore.persist(primary.restore.snapshot());
    expect(settings.get(`${key}.snapshot`)).toBe("");
  });

  it("saves existing primary tabs when restoration is enabled", async () => {
    const { app, restore } = await modules();
    await restore.restore();
    app.addTab({ id: "local", type: "local", label: "Local" });
    await restore.setEnabled(true);
    expect(settings.get(`${key}.snapshot`)).toBe(restore.snapshot());
  });

  it("checks the preference when a queued snapshot starts writing", async () => {
    saved([{ type: "local" }]);
    const { restore } = await modules();
    await restore.restore();
    const blocked = deferred();
    const implementation = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (command, args) => {
      if (command === "set_setting" && args.key === `${key}.snapshot` && args.value === "first") {
        await blocked.promise;
      }
      return implementation(command, args);
    });
    const first = restore.persist("first");
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("set_setting", {
      key: `${key}.snapshot`, value: "first",
    }));
    const second = restore.persist("second");
    settings.set(`${key}.enabled`, "false");
    blocked.resolve();
    await Promise.all([first, second]);
    expect(settings.get(`${key}.snapshot`)).toBe("first");
    expect(invoke).not.toHaveBeenCalledWith("set_setting", { key: `${key}.snapshot`, value: "second" });
  });

  it("keeps the snapshot cleared when disable follows a pending save", async () => {
    saved([{ type: "local" }]);
    const { restore } = await modules();
    await restore.restore();
    const blocked = deferred();
    const implementation = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (command, args) => {
      if (command === "set_setting" && args.key === `${key}.snapshot` && args.value === "pending") {
        await blocked.promise;
      }
      return implementation(command, args);
    });
    const pending = restore.persist("pending");
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("set_setting", {
      key: `${key}.snapshot`, value: "pending",
    }));
    const disable = restore.setEnabled(false);
    const later = restore.persist("later");
    blocked.resolve();
    await Promise.all([pending, disable, later]);
    expect(settings.get(`${key}.enabled`)).toBe("false");
    expect(settings.get(`${key}.snapshot`)).toBe("");
    expect(invoke).not.toHaveBeenCalledWith("set_setting", { key: `${key}.snapshot`, value: "later" });
  });

  it("preserves the snapshot when a preference read fails and saves after recovery", async () => {
    saved([{ type: "local" }]);
    const original = settings.get(`${key}.snapshot`);
    const { restore } = await modules();
    await restore.restore();
    invoke.mockRejectedValueOnce(new Error("settings unavailable"));
    await restore.persist("failed");
    expect(settings.get(`${key}.snapshot`)).toBe(original);
    await restore.persist("recovered");
    expect(settings.get(`${key}.snapshot`)).toBe("recovered");
  });

  it("keeps saved ordering even when MRU reordering is enabled", async () => {
    saved([{ type: "ssh", profileId: "ssh1" }, { type: "local" }], 1);
    const { app, restore } = await modules();
    await app.setTabMru(true);
    await restore.restore();
    expect(app.tabs().map((tab) => tab.type)).toEqual(["home", "ssh", "local"]);
    expect(app.activeTabId()).toBe(app.tabs()[2].id);
  });

  it("uses the shared snapshot on hosts without PTY/serial capabilities", async () => {
    capabilities.localPty = false;
    capabilities.serial = false;
    saved([
      { type: "local" }, { type: "serial", profileId: "s1" },
      { type: "ssh", profileId: "ssh1" }, { type: "telnet", profileId: "telnet1" },
    ], 2);
    const { app, restore } = await modules();
    expect(await restore.restore()).toBe(false);
    expect(app.tabs().map((tab) => tab.type)).toEqual(["home", "ssh", "telnet"]);
    expect(invoke).not.toHaveBeenCalledWith("list_serial_profiles");
    expect(app.activeTabId()).toBe(app.tabs()[1].id);
  });

  it("does not fetch or restore serial profiles on a PTY-only host", async () => {
    capabilities.serial = false;
    saved([{ type: "local" }, { type: "serial", profileId: "serial1" }], 0);
    const { app, restore } = await modules();
    expect(await restore.restore()).toBe(true);
    expect(app.tabs().map((tab) => tab.type)).toEqual(["home", "local"]);
    expect(invoke).not.toHaveBeenCalledWith("list_serial_profiles");
  });
});
