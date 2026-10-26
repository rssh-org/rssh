import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
const capabilities = vi.hoisted(() => ({ localPty: true, serial: true }));
vi.mock("./runtime.svelte.ts", () => ({ capabilities: () => capabilities }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const key = "session_restore";
const actionKey = "startup_action";
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
    if (command === "list_shells") return ["/bin/sh"];
    return null;
  });
});
afterEach(() => vi.unstubAllGlobals());

async function modules() {
  return { app: await import("./app.svelte.ts"), startup: await import("./startup.svelte.ts") };
}
function saved(terminals: unknown[], active = -1) {
  settings.set(actionKey, "restore");
  settings.set(`${key}.snapshot`, JSON.stringify({ version: 1, terminals, active }));
}
function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("startup actions", () => {
  it.each(["home", "local"] as const)("uses %s instead of restoring the saved snapshot", async (action) => {
    saved([{ type: "ssh", profileId: "ssh1" }]);
    settings.set(actionKey, action);
    const { app, startup } = await modules();
    await startup.run();
    expect(app.tabs().map((tab) => tab.type)).toEqual(action === "local" ? ["home", "local"] : ["home"]);
    expect(invoke).not.toHaveBeenCalledWith("list_profiles");
    expect(startup.ready()).toBe(true);
  });

  it("keeps Home when no shell was detected", async () => {
    settings.set(actionKey, "local");
    const implementation = invoke.getMockImplementation()!;
    invoke.mockImplementation((command, args) => command === "list_shells"
      ? Promise.resolve([]) : implementation(command, args));
    const { app, startup } = await modules();
    expect(await startup.availableShells()).toEqual([]);
    await startup.run();
    expect(app.tabs().map((tab) => tab.id)).toEqual(["home"]);
    expect(settings.get(actionKey)).toBe("local");
  });

  it("does not query shells or open local tabs without PTY support", async () => {
    capabilities.localPty = false;
    settings.set(actionKey, "local");
    const { app, startup } = await modules();
    expect(await startup.availableShells()).toEqual([]);
    await startup.run();
    expect(app.tabs().map((tab) => tab.id)).toEqual(["home"]);
    expect(invoke).not.toHaveBeenCalledWith("list_shells");
  });

  it.each(["cancel", "home", "manual"])("does not open a shell after %s during discovery", async (change) => {
    settings.set(actionKey, "local");
    const blocked = deferred();
    const implementation = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (command, args) => {
      if (command === "list_shells") await blocked.promise;
      return implementation(command, args);
    });
    const { app, startup } = await modules();
    const controller = new AbortController();
    const running = startup.run(controller.signal);
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("list_shells"));
    if (change === "cancel") controller.abort();
    if (change === "home") settings.set(actionKey, "home");
    if (change === "manual") app.addTab({ id: "manual", type: "local", label: "Local" });
    blocked.resolve();
    await running;
    expect(app.tabs().map((tab) => tab.id)).toEqual(change === "manual" ? ["home", "manual"] : ["home"]);
  });

  it("does not fall back to a local shell when restoring an empty snapshot", async () => {
    settings.set(actionKey, "restore");
    const { app, startup } = await modules();
    await startup.run();
    expect(app.tabs().map((tab) => tab.id)).toEqual(["home"]);
    expect(invoke).not.toHaveBeenCalledWith("list_shells");
  });

  it("treats an unknown startup action as Home", async () => {
    settings.set(actionKey, "unknown");
    const { app, startup } = await modules();
    expect(await startup.action()).toBe("home");
    await startup.run();
    expect(app.tabs().map((tab) => tab.id)).toEqual(["home"]);
  });

  it("defaults to Home without fetching profiles or reopening tabs", async () => {
    const { app, startup } = await modules();
    await startup.run();
    expect(await startup.action()).toBe("home");
    expect(startup.ready()).toBe(true);
    expect(app.tabs()).toHaveLength(1);
    expect(invoke).not.toHaveBeenCalledWith("list_profiles");
  });

  it("reopens duplicates in order with fresh IDs and current profiles, skipping deleted profiles", async () => {
    saved([
      { type: "ssh", profileId: "ssh1" }, { type: "ssh", profileId: "gone" },
      { type: "local" }, { type: "ssh", profileId: "ssh1" },
      { type: "telnet", profileId: "telnet1" },
    ], 3);
    const { app, startup } = await modules();
    await startup.run();
    expect(app.tabs().map((tab) => tab.type)).toEqual(["home", "ssh", "local", "ssh", "telnet"]);
    expect(new Set(app.tabs().map((tab) => tab.id)).size).toBe(5);
    expect(app.activeTabId()).toBe(app.tabs()[3].id);
    expect(app.tabs()[1].meta).toEqual({ profileId: "ssh1", host: "new-host", port: "2222" });
    expect(app.tabs()[4].meta).not.toHaveProperty("login_script");
  });

  it("persists only root profile references without split panes, secret metadata or unsupported tabs", async () => {
    const { app, startup } = await modules();
    app.addTab({ id: "ssh", type: "ssh", label: "Secret label", meta: { profileId: "ssh1", password: "secret", host: "host" } });
    app.addPane("ssh", "right", { id: "telnet", type: "telnet", label: "Telnet", meta: { profileId: "telnet1", login_script: "secret" } });
    app.addTab({ id: "serial", type: "serial", label: "Serial", meta: { serialProfileId: "serial1", login_script: "secret" } });
    app.addTab({ id: "forward", type: "forward", label: "Forward", meta: { forwardId: "f1" } });
    app.addTab({ id: "container", type: "docker_exec", label: "Container", meta: { connectorSpec: "secret" } });
    app.addTab({ id: "editor", type: "edit", label: "Editor", meta: { content: "secret" } });
    app.setActiveWorkspace("ssh");
    app.setActivePane("telnet");
    const snapshot = startup.snapshot();
    expect(JSON.parse(snapshot)).toEqual({ version: 1, terminals: [
      { type: "ssh", profileId: "ssh1" },
      { type: "serial", profileId: "serial1" },
    ], active: 0 });
    expect(snapshot).not.toContain("secret");
  });

  it("restores only workspace roots and selects the root of the focused split pane", async () => {
    const { app, startup } = await modules();
    app.addTab({ id: "telnet", type: "telnet", label: "Telnet", meta: { profileId: "telnet1" } });
    app.addTab({ id: "ssh", type: "ssh", label: "SSH", meta: { profileId: "ssh1" } });
    app.addPane("ssh", "right", { id: "split-local", type: "local", label: "Local pane" });
    app.addPane("ssh", "bottom", { id: "split-telnet", type: "telnet", label: "Telnet pane", meta: { profileId: "telnet1" } });
    app.addTab({ id: "local", type: "local", label: "Local" });
    app.setActiveWorkspace("ssh");
    app.setActivePane("split-telnet");
    settings.set(actionKey, "restore");
    settings.set(`${key}.snapshot`, startup.snapshot());

    vi.resetModules();
    const restarted = await modules();
    await restarted.startup.run();
    expect(restarted.app.tabs().map((tab) => tab.type)).toEqual(["home", "telnet", "ssh", "local"]);
    expect(restarted.app.activeTabId()).toBe(restarted.app.tabs()[2].id);
    for (const tab of restarted.app.workspaceTabs()) {
      expect(restarted.app.paneIdsForWorkspace(tab.id)).toEqual([tab.id]);
    }
  });

  it.each(["home", "local"] as const)("clears saved sessions when %s is selected and does not persist", async (action) => {
    saved([{ type: "local" }]);
    const { startup } = await modules();
    await startup.run();
    await startup.setAction(action);
    await startup.persist(startup.snapshot());
    expect(settings.get(actionKey)).toBe(action);
    expect(settings.get(`${key}.snapshot`)).toBe("");
  });

  it("records existing terminal tabs without filtering them by host capabilities", async () => {
    const { app, startup } = await modules();
    app.addTab({ id: "local", type: "local", label: "Local" });
    app.addTab({ id: "serial", type: "serial", label: "Serial", meta: { serialProfileId: "serial1" } });
    capabilities.localPty = false;
    capabilities.serial = false;

    expect(JSON.parse(startup.snapshot())).toEqual({ version: 1, terminals: [
      { type: "local" }, { type: "serial", profileId: "serial1" },
    ], active: 1 });
  });

  it("round-trips more than 100 tabs without losing the active terminal", async () => {
    const { app, startup } = await modules();
    for (let index = 0; index < 101; index++) {
      app.addTab({ id: `local:${index}`, type: "local", label: "Local" });
    }
    const snapshot = startup.snapshot();
    expect(JSON.parse(snapshot).terminals).toHaveLength(101);
    settings.set(actionKey, "restore");
    settings.set(`${key}.snapshot`, snapshot);

    vi.resetModules();
    const restarted = await modules();
    await restarted.startup.run();
    expect(restarted.app.tabs()).toHaveLength(102);
    expect(restarted.app.activeTabId()).toBe(restarted.app.tabs()[101].id);
    expect(restarted.startup.snapshot()).toBe(snapshot);
  });

  it.each(["bad JSON", '{"version":2,"terminals":[]}', '{"version":1,"terminals":[{"type":"ssh"}]}'])
    ("handles invalid snapshots: %s", async (snapshot) => {
      settings.set(actionKey, "restore");
      settings.set(`${key}.snapshot`, snapshot);
      const { app, startup } = await modules();
      await startup.run();
      expect(app.tabs()).toHaveLength(1);
    });

  it("does not overwrite the snapshot if profile loading fails", async () => {
    saved([{ type: "ssh", profileId: "ssh1" }]);
    const original = settings.get(`${key}.snapshot`);
    const implementation = invoke.getMockImplementation()!;
    invoke.mockImplementation((command, args) => command === "list_profiles"
      ? Promise.reject(new Error("offline")) : implementation(command, args));
    const { startup } = await modules();
    await startup.run();
    expect(startup.ready()).toBe(false);
    await startup.persist(startup.snapshot());
    expect(settings.get(`${key}.snapshot`)).toBe(original);
  });

  it("does not reopen connections after startup cancellation", async () => {
    saved([{ type: "local" }]);
    const original = settings.get(`${key}.snapshot`);
    const controller = new AbortController();
    controller.abort();
    const { app, startup } = await modules();
    await startup.run(controller.signal);
    expect(app.tabs()).toHaveLength(1);
    expect(startup.ready()).toBe(false);
    await startup.persist(startup.snapshot());
    expect(settings.get(`${key}.snapshot`)).toBe(original);
  });

  it("does not reopen saved connections when disabled during profile loading", async () => {
    saved([{ type: "ssh", profileId: "ssh1" }]);
    const { app, startup } = await modules();
    const blocked = deferred();
    const implementation = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (command, args) => {
      if (command === "list_profiles") await blocked.promise;
      return implementation(command, args);
    });
    const running = startup.run();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("list_profiles"));
    await startup.setAction("home");
    blocked.resolve();
    await running;
    expect(app.tabs().map((tab) => tab.id)).toEqual(["home"]);
    expect(settings.get(`${key}.snapshot`)).toBe("");
  });

  it("does not take over connections opened manually during startup", async () => {
    saved([{ type: "local" }]);
    const { app, startup } = await modules();
    app.addTab({ id: "manual", type: "local", label: "Manual" });
    await startup.run();
    expect(app.tabs().map((tab) => tab.id)).toEqual(["home", "manual"]);
  });

  it("serializes snapshots so the latest write wins", async () => {
    const { startup } = await modules();
    await startup.run();
    await startup.setAction("restore");
    await Promise.all([startup.persist("first"), startup.persist("second")]);
    expect(settings.get(`${key}.snapshot`)).toBe("second");
  });

  it("reads the current startup action instead of caching it", async () => {
    const { startup } = await modules();
    expect(await startup.action()).toBe("home");
    settings.set(actionKey, "restore");
    expect(await startup.action()).toBe("restore");
    settings.set(actionKey, "home");
    expect(await startup.action()).toBe("home");
  });

  it("observes preference changes from a secondary window before saving primary tabs", async () => {
    const primary = await modules();
    await primary.startup.run();
    primary.app.addTab({ id: "primary", type: "local", label: "Primary" });
    vi.resetModules();
    const secondary = await modules();
    secondary.app.addTab({ id: "secondary", type: "local", label: "Secondary" });

    await secondary.startup.setAction("restore");
    expect(secondary.startup.ready()).toBe(false);
    await secondary.startup.persist(secondary.startup.snapshot());
    expect(settings.get(`${key}.snapshot`)).toBeUndefined();
    await primary.startup.persist(primary.startup.snapshot());
    expect(settings.get(`${key}.snapshot`)).toBe(primary.startup.snapshot());

    await secondary.startup.setAction("home");
    await primary.startup.persist(primary.startup.snapshot());
    expect(settings.get(`${key}.snapshot`)).toBe("");
  });

  it("saves existing primary tabs when restoration is selected", async () => {
    const { app, startup } = await modules();
    await startup.run();
    app.addTab({ id: "local", type: "local", label: "Local" });
    await startup.setAction("restore");
    expect(settings.get(`${key}.snapshot`)).toBe(startup.snapshot());
  });

  it("checks the preference when a queued snapshot starts writing", async () => {
    saved([{ type: "local" }]);
    const { startup } = await modules();
    await startup.run();
    const blocked = deferred();
    const implementation = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (command, args) => {
      if (command === "set_setting" && args.key === `${key}.snapshot` && args.value === "first") {
        await blocked.promise;
      }
      return implementation(command, args);
    });
    const first = startup.persist("first");
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("set_setting", {
      key: `${key}.snapshot`, value: "first",
    }));
    const second = startup.persist("second");
    settings.set(actionKey, "home");
    blocked.resolve();
    await Promise.all([first, second]);
    expect(settings.get(`${key}.snapshot`)).toBe("first");
    expect(invoke).not.toHaveBeenCalledWith("set_setting", { key: `${key}.snapshot`, value: "second" });
  });

  it("keeps the snapshot cleared when disable follows a pending save", async () => {
    saved([{ type: "local" }]);
    const { startup } = await modules();
    await startup.run();
    const blocked = deferred();
    const implementation = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (command, args) => {
      if (command === "set_setting" && args.key === `${key}.snapshot` && args.value === "pending") {
        await blocked.promise;
      }
      return implementation(command, args);
    });
    const pending = startup.persist("pending");
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("set_setting", {
      key: `${key}.snapshot`, value: "pending",
    }));
    const disable = startup.setAction("home");
    const later = startup.persist("later");
    blocked.resolve();
    await Promise.all([pending, disable, later]);
    expect(settings.get(actionKey)).toBe("home");
    expect(settings.get(`${key}.snapshot`)).toBe("");
    expect(invoke).not.toHaveBeenCalledWith("set_setting", { key: `${key}.snapshot`, value: "later" });
  });

  it("preserves the snapshot when a preference read fails and saves after recovery", async () => {
    saved([{ type: "local" }]);
    const original = settings.get(`${key}.snapshot`);
    const { startup } = await modules();
    await startup.run();
    invoke.mockRejectedValueOnce(new Error("settings unavailable"));
    await startup.persist("failed");
    expect(settings.get(`${key}.snapshot`)).toBe(original);
    await startup.persist("recovered");
    expect(settings.get(`${key}.snapshot`)).toBe("recovered");
  });

  it("keeps saved ordering even when MRU reordering is enabled", async () => {
    saved([{ type: "ssh", profileId: "ssh1" }, { type: "local" }], 1);
    const { app, startup } = await modules();
    await app.setTabMru(true);
    await startup.run();
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
    const { app, startup } = await modules();
    await startup.run();
    expect(app.tabs().map((tab) => tab.type)).toEqual(["home", "ssh", "telnet"]);
    expect(invoke).not.toHaveBeenCalledWith("list_serial_profiles");
    expect(app.activeTabId()).toBe(app.tabs()[1].id);
  });

  it("does not fetch or restore serial profiles on a PTY-only host", async () => {
    capabilities.serial = false;
    saved([{ type: "local" }, { type: "serial", profileId: "serial1" }], 0);
    const { app, startup } = await modules();
    await startup.run();
    expect(app.tabs().map((tab) => tab.type)).toEqual(["home", "local"]);
    expect(invoke).not.toHaveBeenCalledWith("list_serial_profiles");
  });
});
