import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DynamicDiscoverySource, DynamicDiscoverySnapshot } from "./app.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

let discovery: typeof import("./discovery.svelte.ts");
const preferences = { auto_refresh: true, interval_seconds: 60 };
const snapshot = { targets: [], errors: [] };
const source: DynamicDiscoverySource = { id: "aws", name: "AWS", enabled: true, platform: "aws_ssm", profile: "", region: "us-east-1", only_ssm_available: false };

function discoverCalls() {
  return invokeMock.mock.calls.filter(([command]) => command === "discover_dynamic_targets");
}

function sourceNotifications() {
  return vi.mocked(localStorage.setItem).mock.calls.filter(([key]) => key === "dynamic_discovery_sources.changed");
}

async function connectOtherWindow() {
  const events = new EventTarget();
  vi.stubGlobal("window", events);
  vi.mocked(localStorage.setItem).mockImplementation((key, newValue) => {
    events.dispatchEvent(Object.assign(new Event("storage"), { key, newValue }));
  });
  invokeMock.mockImplementation(async (command) => {
    if (command === "get_setting") return JSON.stringify({ ...preferences, auto_refresh: false });
    if (command === "discover_dynamic_targets") return snapshot;
  });
  await discovery.startBackgroundRefresh();
  vi.resetModules();
  return { events, otherWindow: await import("./discovery.svelte.ts") };
}

beforeEach(async () => {
  vi.resetModules();
  vi.useFakeTimers();
  vi.stubGlobal("localStorage", { getItem: () => null, setItem: vi.fn() });
  invokeMock.mockReset().mockImplementation(async (command) => {
    if (command === "get_setting") return JSON.stringify(preferences);
    if (command === "discover_dynamic_targets") return snapshot;
  });
  discovery = await import("./discovery.svelte.ts");
});

afterEach(() => {
  discovery?.stopBackgroundRefresh();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("dynamic discovery refresh", () => {
  it.each(["delete", "disable", "edit"] as const)("invalidates another window's %s source with auto refresh off and preserves unrelated results", async (operation) => {
    const { otherWindow } = await connectOtherWindow();
    const keep = { ...source, id: "keep" };
    const cached = {
      targets: [source, keep].map(({ id }) => ({ id, source_id: id })),
      errors: [source, keep].map(({ id }) => ({ source_id: id, message: "old failure" })),
    };
    invokeMock.mockResolvedValueOnce(cached);
    await discovery.refresh();
    const replacement = operation === "disable" ? { ...source, enabled: false } : { ...source, region: "eu-west-1" };
    otherWindow.sourcesChanged([source, keep], operation === "delete" ? [keep] : [replacement, keep]);

    expect(discovery.targets()).toEqual([cached.targets[1]]);
    expect(discovery.errors()).toEqual([cached.errors[1]]);
    expect(sourceNotifications()).toHaveLength(1); // Receivers never rebroadcast.
    expect(JSON.parse(sourceNotifications()[0][1])).toEqual({ ids: [source.id], nonce: expect.any(String) });
    await vi.advanceTimersByTimeAsync(180_000);
    expect(discoverCalls()).toHaveLength(1);
    expect(discovery.preferences().auto_refresh).toBe(false);
  });

  it.each([false, true])("discards another window's in-flight discovery after a source edit (stale failure=%s)", async (fails) => {
    const { otherWindow } = await connectOtherWindow();
    const keep = { ...source, id: "keep" };
    const cached = {
      targets: [source, keep].map(({ id }) => ({ id, source_id: id })),
      errors: [source, keep].map(({ id }) => ({ source_id: id, message: "old failure" })),
    };
    invokeMock.mockResolvedValueOnce(cached);
    await discovery.refresh();
    let finish!: (value: unknown) => void;
    let reject!: (error: Error) => void;
    invokeMock.mockImplementationOnce(() => new Promise((resolve, fail) => { finish = resolve; reject = fail; }));
    const old = discovery.refresh().catch(() => snapshot);
    otherWindow.sourcesChanged([source, keep], [keep]);
    if (fails) reject(new Error("stale source failure"));
    else finish(cached);
    await old;

    expect(discovery.targets()).toEqual([cached.targets[1]]);
    expect(discovery.errors()).toEqual([cached.errors[1]]);
    expect(discovery.failure()).toBeNull();
    await vi.advanceTimersByTimeAsync(180_000);
    expect(discoverCalls()).toHaveLength(2);
    expect(sourceNotifications()).toHaveLength(1);
  });

  it("notifies repeated edits to the same IDs with distinct nonces", async () => {
    const { otherWindow } = await connectOtherWindow();
    const cached = { targets: [{ id: "target", source_id: source.id }], errors: [] };
    const renamed = { ...source, name: "Renamed" };
    invokeMock.mockResolvedValueOnce(cached);
    await discovery.refresh();
    otherWindow.sourcesChanged([source], [renamed]);
    expect(discovery.targets()).toEqual([]);
    invokeMock.mockResolvedValueOnce(cached);
    await discovery.refresh();
    otherWindow.sourcesChanged([renamed], [{ ...renamed, region: "eu-west-1" }]);
    expect(discovery.targets()).toEqual([]);

    const notifications = sourceNotifications().map(([, value]) => JSON.parse(value));
    expect(notifications).toHaveLength(2);
    expect(notifications[0].ids).toEqual(notifications[1].ids);
    expect(notifications[0].nonce).not.toBe(notifications[1].nonce);
    expect(discoverCalls()).toHaveLength(2);
  });

  it("invalidates in-flight discovery for an added source without removing unchanged cached results", async () => {
    const { otherWindow } = await connectOtherWindow();
    const cached = { targets: [{ id: "cached", source_id: source.id }], errors: [] };
    invokeMock.mockResolvedValueOnce(cached);
    await discovery.refresh();
    let finish!: (value: unknown) => void;
    invokeMock.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const old = discovery.refresh();
    otherWindow.sourcesChanged([source], [source, { ...source, id: "added" }]);
    expect(JSON.parse(sourceNotifications()[0][1]).ids).toEqual([]);
    finish({ targets: [{ id: "stale", source_id: source.id }], errors: [] });
    await old;
    expect(discovery.targets()).toEqual(cached.targets);
    expect(discoverCalls()).toHaveLength(2);
  });

  it("does not notify or invalidate another window when an unchanged source is saved", async () => {
    const { otherWindow } = await connectOtherWindow();
    let finish!: (value: unknown) => void;
    invokeMock.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const request = discovery.refresh();
    const { region, ...rest } = source;
    otherWindow.sourcesChanged([source], [{ region, ...rest }]);
    const latest = { targets: [{ id: "latest", source_id: source.id }], errors: [] };
    finish(latest);
    await request;
    expect(discovery.targets()).toEqual(latest.targets);
    expect(sourceNotifications()).toHaveLength(0);
  });

  it("removes the source notification listener on teardown", async () => {
    const { otherWindow } = await connectOtherWindow();
    const cached = { targets: [{ id: "target", source_id: source.id }], errors: [] };
    invokeMock.mockResolvedValueOnce(cached);
    await discovery.refresh();
    discovery.stopBackgroundRefresh();
    invokeMock.mockClear();
    otherWindow.sourcesChanged([source], []);
    await vi.advanceTimersByTimeAsync(180_000);
    expect(discovery.targets()).toEqual(cached.targets);
    expect(invokeMock).not.toHaveBeenCalled();
    expect(sourceNotifications()).toHaveLength(1);
  });

  it("ignores malformed source notifications without invalidating an in-flight result", async () => {
    const { events } = await connectOtherWindow();
    let finish!: (value: unknown) => void;
    invokeMock.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const request = discovery.refresh();
    for (const value of [null, "null", "[]", '{"ids":[1],"nonce":"x"}', '{"ids":["aws"]}', '{"ids":"aws","nonce":"x"}']) {
      events.dispatchEvent(Object.assign(new Event("storage"), { key: "dynamic_discovery_sources.changed", newValue: value }));
    }
    const latest = { targets: [{ id: "latest", source_id: source.id }], errors: [] };
    finish(latest);
    await request;
    expect(discovery.targets()).toEqual(latest.targets);
    expect(sourceNotifications()).toHaveLength(0);
  });

  it("reloads another window's saved refresh preferences and removes its listener on teardown", async () => {
    const events = new EventTarget();
    vi.stubGlobal("window", events);
    vi.mocked(localStorage.setItem).mockImplementation((key) => {
      events.dispatchEvent(Object.assign(new Event("storage"), { key }));
    });
    let saved = { ...preferences };
    invokeMock.mockImplementation(async (command, args) => {
      if (command === "get_setting") return JSON.stringify(saved);
      if (command === "set_setting") saved = JSON.parse(args.value);
      if (command === "discover_dynamic_targets") return snapshot;
    });
    await discovery.startBackgroundRefresh();
    vi.resetModules();
    const otherWindow = await import("./discovery.svelte.ts");

    await otherWindow.savePreferences({ auto_refresh: false, interval_seconds: 60 });
    await vi.advanceTimersByTimeAsync(180_000);
    expect(discovery.preferences().auto_refresh).toBe(false);
    expect(discoverCalls()).toHaveLength(1);

    await otherWindow.savePreferences({ auto_refresh: true, interval_seconds: 120 });
    await vi.advanceTimersByTimeAsync(0);
    expect(discovery.preferences()).toEqual({ auto_refresh: true, interval_seconds: 120 });
    expect(discoverCalls()).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(120_000);
    expect(discoverCalls()).toHaveLength(2);

    await otherWindow.savePreferences({ auto_refresh: true, interval_seconds: 10 });
    await vi.advanceTimersByTimeAsync(9_999);
    expect(discoverCalls()).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(discoverCalls()).toHaveLength(3);

    discovery.stopBackgroundRefresh();
    invokeMock.mockClear();
    await otherWindow.savePreferences({ auto_refresh: false, interval_seconds: 10 });
    await vi.advanceTimersByTimeAsync(180_000);
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(["set_setting"]);
  });

  it("does not broadcast a preference change when the DB save fails", async () => {
    await discovery.startBackgroundRefresh();
    invokeMock.mockRejectedValueOnce(new Error("read-only settings"));
    await expect(discovery.savePreferences({ ...preferences, auto_refresh: false })).rejects.toThrow("read-only settings");
    expect(localStorage.setItem).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(discoverCalls()).toHaveLength(2);
  });

  it.each([false, true])("discards an older settings read after another window disables refresh (stopped=%s)", async (stopped) => {
    const events = new EventTarget();
    vi.stubGlobal("window", events);
    vi.mocked(localStorage.setItem).mockImplementation((key) => {
      events.dispatchEvent(Object.assign(new Event("storage"), { key }));
    });
    let finishOldRead!: (raw: string) => void;
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_setting") return JSON.stringify({ ...preferences, auto_refresh: false });
      if (command === "discover_dynamic_targets") return snapshot;
    });
    invokeMock.mockImplementationOnce(() => new Promise((resolve) => { finishOldRead = resolve; }));
    const starting = discovery.startBackgroundRefresh();
    vi.resetModules();
    const otherWindow = await import("./discovery.svelte.ts");
    await otherWindow.savePreferences({ ...preferences, auto_refresh: false });
    if (stopped) discovery.stopBackgroundRefresh();
    finishOldRead(JSON.stringify(preferences));
    await starting;
    await vi.advanceTimersByTimeAsync(180_000);
    if (stopped) expect(discovery.loaded()).toBe(false);
    else expect(discovery.preferences().auto_refresh).toBe(false);
    expect(invokeMock.mock.calls.filter(([command]) => command === "get_setting")).toHaveLength(stopped ? 1 : 2);
    expect(discoverCalls()).toHaveLength(0);
  });

  it("recovers automatic refresh after a failed settings read and a later preference save", async () => {
    invokeMock.mockRejectedValueOnce(new Error("database temporarily unavailable"));
    await expect(discovery.startBackgroundRefresh()).rejects.toThrow("database temporarily unavailable");
    await discovery.loadPreferences();
    await discovery.savePreferences(preferences);
    await vi.advanceTimersByTimeAsync(0);
    expect(discoverCalls()).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(discoverCalls()).toHaveLength(2);
  });

  it("allows startup to retry a failed settings read", async () => {
    invokeMock.mockRejectedValueOnce(new Error("database temporarily unavailable"));
    await expect(discovery.startBackgroundRefresh()).rejects.toThrow();
    await discovery.startBackgroundRefresh();
    expect(discoverCalls()).toHaveLength(1);
  });

  it("resumes the timer when the settings page recovers a failed preference load", async () => {
    invokeMock.mockRejectedValueOnce(new Error("database temporarily unavailable"));
    await expect(discovery.startBackgroundRefresh()).rejects.toThrow();
    await discovery.loadPreferences();
    expect(discoverCalls()).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(discoverCalls()).toHaveLength(1);
  });

  it("loads settings without discovering, then runs one application-wide schedule", async () => {
    await discovery.loadPreferences();
    expect(discoverCalls()).toHaveLength(0);
    await discovery.startBackgroundRefresh();
    await discovery.startBackgroundRefresh();
    expect(discoverCalls()).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(59_999);
    expect(discoverCalls()).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(discoverCalls()).toHaveLength(2);
  });

  it("does not automatically discover with auto refresh disabled, but manual refresh publishes results", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_setting") return JSON.stringify({ ...preferences, auto_refresh: false });
      if (command === "discover_dynamic_targets") return { targets: [{ id: "example" }], errors: [] };
    });
    await discovery.startBackgroundRefresh();
    await vi.advanceTimersByTimeAsync(180_000);
    expect(discoverCalls()).toHaveLength(0);
    await discovery.refresh();
    expect(discovery.targets()).toEqual([{ id: "example" }]);
    await vi.advanceTimersByTimeAsync(180_000);
    expect(discoverCalls()).toHaveLength(1);
  });

  it("does not schedule from a manual refresh before saved preferences finish loading", async () => {
    let finishPreferences!: (value: string) => void;
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_setting") return new Promise((resolve) => { finishPreferences = resolve; });
      if (command === "discover_dynamic_targets") return snapshot;
    });
    const starting = discovery.startBackgroundRefresh();
    await discovery.refresh();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(discoverCalls()).toHaveLength(1);
    finishPreferences(JSON.stringify({ ...preferences, auto_refresh: false }));
    await starting;
    await vi.advanceTimersByTimeAsync(120_000);
    expect(discoverCalls()).toHaveLength(1);
  });

  it("starts the schedule after a source changes while preferences are loading", async () => {
    let finishPreferences!: (value: string) => void;
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_setting") return new Promise((resolve) => { finishPreferences = resolve; });
      if (command === "discover_dynamic_targets") return snapshot;
    });
    const starting = discovery.startBackgroundRefresh();
    discovery.sourcesChanged([source], [{ ...source, region: "eu-west-1" }]);
    expect(discoverCalls()).toHaveLength(0);
    finishPreferences(JSON.stringify(preferences));
    await starting;
    expect(discoverCalls()).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(discoverCalls()).toHaveLength(2);
  });

  it("shares in-flight manual and automatic refreshes and waits a full interval after completion", async () => {
    let finish!: (value: typeof snapshot) => void;
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_setting") return JSON.stringify(preferences);
      if (command === "discover_dynamic_targets") return new Promise((resolve) => { finish = resolve; });
    });
    const starting = discovery.startBackgroundRefresh();
    await vi.advanceTimersByTimeAsync(0);
    const manual = discovery.refresh();
    await vi.advanceTimersByTimeAsync(180_000);
    expect(discoverCalls()).toHaveLength(1);
    expect(discovery.refreshing()).toBe(true);
    finish(snapshot);
    await Promise.all([starting, manual]);
    expect(discovery.refreshing()).toBe(false);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(discoverCalls()).toHaveLength(2);
    finish(snapshot);
    await vi.advanceTimersByTimeAsync(0);
  });

  it("persists interval changes and cancels scheduled refreshes when disabled", async () => {
    await discovery.startBackgroundRefresh();
    await discovery.savePreferences({ auto_refresh: true, interval_seconds: 120 });
    await vi.advanceTimersByTimeAsync(0);
    const afterSave = discoverCalls().length;
    await vi.advanceTimersByTimeAsync(119_999);
    expect(discoverCalls()).toHaveLength(afterSave);
    await vi.advanceTimersByTimeAsync(1);
    expect(discoverCalls()).toHaveLength(afterSave + 1);
    await discovery.savePreferences({ auto_refresh: false, interval_seconds: 120 });
    await vi.advanceTimersByTimeAsync(600_000);
    expect(discoverCalls()).toHaveLength(afterSave + 1);
    expect(invokeMock).toHaveBeenCalledWith("set_setting", {
      key: "dynamic_discovery_refresh",
      value: JSON.stringify({ auto_refresh: false, interval_seconds: 120 }),
    });
  });

  it("retains the last snapshot on transport failure and allows the next refresh", async () => {
    await discovery.loadPreferences();
    invokeMock.mockResolvedValueOnce({ targets: [{ id: "last-good" }], errors: [] });
    await discovery.refresh();
    invokeMock.mockRejectedValueOnce(new Error("offline"));
    await expect(discovery.refresh()).rejects.toThrow("offline");
    expect(discovery.targets()).toEqual([{ id: "last-good" }]);
    expect(discovery.failure()).toBeInstanceOf(Error);
    await discovery.refresh();
    expect(discovery.failure()).toBeNull();
  });

  it("publishes partial source failures without hiding their error details", async () => {
    await discovery.loadPreferences();
    const partial = { targets: [{ id: "docker" }], errors: [{ source_id: "aws", message: "denied" }] };
    invokeMock.mockResolvedValueOnce(partial);
    await discovery.refresh();
    expect(discovery.targets()).toEqual(partial.targets);
    expect(discovery.errors()).toEqual(partial.errors);
  });

  it("removes cached targets and errors only for deleted, disabled or edited sources without discovering", async () => {
    const previous = ["keep", "deleted", "disabled", "renamed"].map((id) => ({ ...source, id }));
    const cached = {
      targets: previous.map(({ id }) => ({ id, source_id: id })),
      errors: previous.map(({ id }) => ({ source_id: id, message: "old failure" })),
    };
    invokeMock.mockResolvedValueOnce(cached);
    await discovery.refresh();
    const next = [previous[0], { ...previous[2], enabled: false }, { ...previous[3], name: "New label" }];
    discovery.sourcesChanged(previous, next);

    expect(discovery.targets()).toEqual([cached.targets[0]]);
    expect(discovery.errors()).toEqual([cached.errors[0]]);
    expect(discoverCalls()).toHaveLength(1);
  });

  it.each([false, true])("waits for an invalidated request before issuing one fresh manual refresh (old failure=%s)", async (oldFails) => {
    let finish!: (value: unknown) => void;
    let reject!: (error: Error) => void;
    invokeMock.mockImplementation(() => new Promise((resolve, fail) => { finish = resolve; reject = fail; }));
    const old = discovery.refresh().catch(() => snapshot);
    discovery.sourcesChanged([source], [{ ...source, region: "eu-west-1" }]);
    const manual = discovery.refresh();
    const secondManual = discovery.refresh();
    expect(discoverCalls()).toHaveLength(1);
    if (oldFails) reject(new Error("stale failure"));
    else finish({ targets: [{ id: "old", source_id: source.id }], errors: [] });
    await vi.advanceTimersByTimeAsync(0);
    expect(discoverCalls()).toHaveLength(2);
    expect(discovery.targets()).toEqual([]);
    expect(discovery.failure()).toBeNull();
    const latest = { targets: [{ id: "latest", source_id: source.id }], errors: [] };
    finish(latest);
    await Promise.all([old, manual, secondManual]);
    expect(discovery.targets()).toEqual(latest.targets);
  });

  it("keeps an in-flight result when an unchanged source is saved in a different field order", async () => {
    let finish!: (value: DynamicDiscoverySnapshot) => void;
    invokeMock.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    const request = discovery.refresh();
    const { region, ...rest } = source;
    discovery.sourcesChanged([source], [{ region, ...rest }]);
    finish({ targets: [{ id: "kept" }] as never[], errors: [] });
    await request;
    expect(discovery.targets()).toEqual([{ id: "kept" }]);
  });

  it("does not start a queued manual refresh after application teardown", async () => {
    let finish!: (value: typeof snapshot) => void;
    invokeMock.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    const old = discovery.refresh();
    discovery.sourcesChanged([source], []);
    const queued = discovery.refresh();
    discovery.stopBackgroundRefresh();
    finish(snapshot);
    await Promise.all([old, queued]);
    expect(discoverCalls()).toHaveLength(1);
  });

  it("rejects invalid intervals without writing settings or starting discovery", async () => {
    await discovery.loadPreferences();
    for (const interval_seconds of [0, -1, 1.5, NaN, Infinity, 86_401]) {
      await expect(discovery.savePreferences({ auto_refresh: true, interval_seconds })).rejects.toThrow();
    }
    expect(invokeMock.mock.calls.some(([command]) => command === "set_setting")).toBe(false);
    expect(discoverCalls()).toHaveLength(0);
  });

  it("does not publish an in-flight result or reschedule after teardown", async () => {
    let finish!: (value: typeof snapshot) => void;
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_setting") return JSON.stringify(preferences);
      return new Promise((resolve) => { finish = resolve; });
    });
    const starting = discovery.startBackgroundRefresh();
    await vi.advanceTimersByTimeAsync(0);
    discovery.stopBackgroundRefresh();
    finish({ targets: [{ id: "stale" }] as never[], errors: [] });
    await starting;
    expect(discovery.targets()).toEqual([]);
    await vi.advanceTimersByTimeAsync(180_000);
    expect(discoverCalls()).toHaveLength(1);
  });
});
