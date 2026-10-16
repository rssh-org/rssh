import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Group } from "../stores/app.svelte.ts";

const { subscribe } = vi.hoisted(() => ({ subscribe: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: subscribe, emit: vi.fn() }));
vi.mock("../stores/sync.svelte.ts", () => ({ invalidateConfiguration: vi.fn() }));
import { watchEnvironmentGroup } from "./watch-environment-group";

const listeners = new Map<string, Set<() => void>>();
function dispatch(event: string) { for (const callback of [...(listeners.get(event) ?? [])]) callback(); }
const group = (id: string, color: string): Group => ({
  id, name: id, color, sort_order: 0, terminal_background: color,
});

beforeEach(() => {
  listeners.clear();
  subscribe.mockReset().mockImplementation(async (event: string, callback: () => void) => {
    const callbacks = listeners.get(event) ?? new Set();
    listeners.set(event, callbacks);
    callbacks.add(callback);
    return () => callbacks.delete(callback);
  });
});

describe("environment group subscriptions", () => {
  it("updates independent terminal instances on a resource-scoped group event", async () => {
    let saved = group("g1", "#FF0000");
    const first = vi.fn(), second = vi.fn(), failed = vi.fn();
    const off = [first, second].map(changed => watchEnvironmentGroup("p1",
      async () => ({group_id: "g1"}), async () => [{...saved}], changed, failed));
    await vi.waitFor(() => expect(second).toHaveBeenLastCalledWith(saved));
    saved = {...saved, color: "#00FF00", name: "renamed", terminal_background: null};
    dispatch("group:changed:g1");
    await vi.waitFor(() => {
      expect(first).toHaveBeenLastCalledWith(saved);
      expect(second).toHaveBeenLastCalledWith(saved);
    });
    expect(failed).not.toHaveBeenCalled();
    off.forEach(stop => stop());
    expect([...listeners.values()].every(callbacks => callbacks.size === 0)).toBe(true);
  });

  it("switches group subscriptions when another window reassigns the connection", async () => {
    let membership: string | null = "g1";
    const saved = [group("g1", "#FF0000"), group("g2", "#0000FF")];
    const changed = vi.fn();
    const stop = watchEnvironmentGroup("p1", async () => ({group_id: membership}), async () => saved, changed, vi.fn());
    await vi.waitFor(() => expect(changed).toHaveBeenLastCalledWith(saved[0]));
    membership = "g2";
    dispatch("connection:changed:p1");
    await vi.waitFor(() => expect(changed).toHaveBeenLastCalledWith(saved[1]));
    expect(listeners.get("group:changed:g1")?.size).toBe(0);
    expect(listeners.get("group:changed:g2")?.size).toBe(1);
    membership = null; // group deletion also clears the profile's membership
    dispatch("group:changed:g2");
    await vi.waitFor(() => expect(changed).toHaveBeenLastCalledWith(null));
    expect(listeners.get("group:changed:g2")?.size).toBe(0);
    stop();
  });

  it("unsubscribes even if the pane closes before listen finishes", async () => {
    let complete!: (off: () => void) => void;
    subscribe.mockReturnValueOnce(new Promise(resolve => { complete = resolve; }));
    const read = vi.fn(), off = vi.fn();
    const stop = watchEnvironmentGroup("p1", read, vi.fn(), vi.fn(), vi.fn());
    stop();
    complete(off);
    await vi.waitFor(() => expect(off).toHaveBeenCalledOnce());
    expect(read).not.toHaveBeenCalled();
  });

  it("ignores stale reads after a newer change notification", async () => {
    let complete!: (groups: Group[]) => void;
    const readGroups = vi.fn().mockReturnValueOnce(new Promise(resolve => { complete = resolve; }))
      .mockResolvedValue([group("g1", "#00FF00")]);
    const changed = vi.fn();
    const stop = watchEnvironmentGroup("p1", async () => ({group_id: "g1"}), readGroups, changed, vi.fn());
    await vi.waitFor(() => expect(readGroups).toHaveBeenCalledOnce());
    dispatch("group:changed:g1");
    await vi.waitFor(() => expect(changed).toHaveBeenLastCalledWith(group("g1", "#00FF00")));
    complete([group("g1", "#FF0000")]);
    await Promise.resolve();
    expect(changed).toHaveBeenCalledOnce();
    stop();
  });
});
