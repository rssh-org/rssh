import { describe, expect, it } from "vitest";
import { createHomeRefresh } from "./home-refresh.ts";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

describe("Home static data refresh", () => {
  it("publishes the loaded saved connections", async () => {
    const applied: string[] = [];
    const refresher = createHomeRefresh({ loadStatic: async () => "profiles", applyStatic: (value) => applied.push(value) });
    await refresher.refresh();
    expect(applied).toEqual(["profiles"]);
  });

  it("does not let an older result overwrite a newer refresh", async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const loads = [first, second];
    const applied: string[] = [];
    const refresher = createHomeRefresh({ loadStatic: () => loads.shift()!.promise, applyStatic: (value) => applied.push(value) });
    const stale = refresher.refresh();
    const current = refresher.refresh();
    second.resolve("new");
    await current;
    first.resolve("old");
    await stale;
    expect(applied).toEqual(["new"]);
  });

  it("does not publish a pending result after leaving Home", async () => {
    const pending = deferred<string>();
    const applied: string[] = [];
    const refresher = createHomeRefresh({ loadStatic: () => pending.promise, applyStatic: (value) => applied.push(value) });
    const refreshing = refresher.refresh();
    refresher.cancel();
    pending.resolve("stale");
    await refreshing;
    expect(applied).toEqual([]);
  });

  it("reports current failures but ignores stale and cancelled failures", async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const third = deferred<string>();
    const loads = [first, second, third];
    const errors: unknown[] = [];
    const refresher = createHomeRefresh({ loadStatic: () => loads.shift()!.promise, applyStatic: () => {}, onError: (error) => errors.push(error) });
    const stale = refresher.refresh();
    const current = refresher.refresh();
    first.reject(new Error("stale"));
    const failure = new Error("current");
    second.reject(failure);
    await Promise.all([stale, current]);
    const cancelled = refresher.refresh();
    refresher.cancel();
    third.reject(new Error("cancelled"));
    await cancelled;
    expect(errors).toEqual([failure]);
  });
});
