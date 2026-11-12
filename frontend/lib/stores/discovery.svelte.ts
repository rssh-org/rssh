import { invoke } from "@tauri-apps/api/core";
import type { DynamicDiscoverySnapshot, DynamicDiscoverySource } from "./app.svelte.ts";
import { t } from "../i18n/index.svelte.ts";

export interface DiscoveryRefreshPreferences {
  auto_refresh: boolean;
  interval_seconds: number;
}

const SETTING_KEY = "dynamic_discovery_refresh";
const PREFERENCES_CHANGED_KEY = `${SETTING_KEY}.changed`;
const SOURCES_CHANGED_KEY = "dynamic_discovery_sources.changed";
let _preferences = $state<DiscoveryRefreshPreferences>({ auto_refresh: true, interval_seconds: 60 });
let _loaded = $state(false);
let _snapshot = $state<DynamicDiscoverySnapshot>({ targets: [], errors: [] });
let _refreshing = $state(false);
let _failure = $state<unknown>(null);
let loading: Promise<void> | null = null;
let pending: { generation: number; promise: Promise<DynamicDiscoverySnapshot> } | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;
let running = false;
let generation = 0;
let runGeneration = 0;
let preferencesRevision = 0;

export function preferences(): Readonly<DiscoveryRefreshPreferences> { return _preferences; }
export function loaded(): boolean { return _loaded; }
export function targets() { return _snapshot.targets; }
export function errors() { return _snapshot.errors; }
export function refreshing(): boolean { return _refreshing; }
export function failure(): unknown { return _failure; }

function validInterval(value: number): boolean {
  return Number.isInteger(value) && value >= 1 && value <= 86_400;
}

export function loadPreferences(): Promise<void> {
  if (_loaded) return Promise.resolve();
  if (loading) return loading;
  const revision = preferencesRevision;
  loading = invoke<string | null>("get_setting", { key: SETTING_KEY }).then((raw) => {
    if (revision !== preferencesRevision) return;
    if (raw !== null) {
      let value: DiscoveryRefreshPreferences;
      try {
        value = JSON.parse(raw);
      } catch {
        throw new Error(t("dynamic_discovery.error.refresh_settings_invalid"));
      }
      if (!value || typeof value.auto_refresh !== "boolean" || !validInterval(value.interval_seconds)) {
        throw new Error(t("dynamic_discovery.error.refresh_settings_invalid"));
      }
      _preferences = { auto_refresh: value.auto_refresh, interval_seconds: value.interval_seconds };
    } else {
      _preferences = { auto_refresh: true, interval_seconds: 60 };
    }
    _loaded = true;
    scheduleNext();
  }).finally(() => { loading = null; });
  return loading;
}

function onStorageChanged(event: StorageEvent): void {
  if (!running) return;
  if (event.key === SOURCES_CHANGED_KEY) {
    if (event.newValue === null) return;
    try {
      const change: unknown = JSON.parse(event.newValue);
      if (!change || typeof change !== "object"
        || !("ids" in change) || !Array.isArray(change.ids)
        || !change.ids.every((id): id is string => typeof id === "string")
        || !("nonce" in change) || typeof change.nonce !== "string") return;
      invalidateSources(new Set(change.ids));
    } catch (error) {
      console.warn("[dynamic-discovery] source change notification invalid:", error);
    }
    return;
  }
  if (event.key !== PREFERENCES_CHANGED_KEY) return;
  preferencesRevision += 1;
  _loaded = false;
  clearTimer();
  const current = runGeneration;
  // A notification carries no preferences. Read the DB after any stale read
  // settles, then schedule using the same policy as a normal preference load.
  const reload = () => running && current === runGeneration ? loadPreferences() : Promise.resolve();
  void (loading ? loading.then(reload, reload) : reload()).catch((error) => {
    console.warn("[dynamic-discovery] refresh settings reload failed:", error);
  });
}

function clearTimer(): void {
  if (timer !== null) clearTimeout(timer);
  timer = null;
}

function scheduleNext(): void {
  clearTimer();
  if (!running || !_loaded || !_preferences.auto_refresh) return;
  timer = setTimeout(() => {
    timer = null;
    if (running && _loaded && _preferences.auto_refresh) void refreshAutomatically();
  }, _preferences.interval_seconds * 1000);
}

async function refreshAutomatically(): Promise<void> {
  try {
    await refresh();
  } catch (error) {
    // Keep the last snapshot; the settings block exposes the current failure.
    console.warn("[dynamic-discovery] automatic refresh failed:", error);
  }
}

/** Manual and scheduled refreshes share one request and one result across pages. */
export function refresh(): Promise<DynamicDiscoverySnapshot> {
  if (pending) {
    if (pending.generation === generation) return pending.promise;
    const requested = generation;
    // A changed source invalidates the old request. Wait for it to finish
    // before querying the new configuration, even if the old request failed.
    const retry = () => requested === generation ? refresh() : _snapshot;
    return pending.promise.then(retry, retry);
  }
  clearTimer();
  const current = generation;
  _refreshing = true;
  const promise = invoke<DynamicDiscoverySnapshot>("discover_dynamic_targets").then((snapshot) => {
    if (current === generation) {
      _snapshot = snapshot;
      _failure = null;
    }
    return snapshot;
  }).catch((error) => {
    if (current === generation) _failure = error;
    throw error;
  }).finally(() => {
    pending = null;
    _refreshing = false;
    scheduleNext();
  });
  pending = { generation: current, promise };
  return promise;
}

function invalidateSources(changed: Set<string>): void {
  generation += 1;
  _snapshot = {
    targets: _snapshot.targets.filter((target) => !changed.has(target.source_id)),
    errors: _snapshot.errors.filter((error) => !changed.has(error.source_id)),
  };
}

/** Called after saving sources. Invalidate every window's cached results without
 * triggering discovery; notifications carry source IDs, never discovered data. */
export function sourcesChanged(previous: DynamicDiscoverySource[], next: DynamicDiscoverySource[]): void {
  const updated = new Map(next.map((source) => [source.id, source]));
  const changed = new Set(previous.filter((source) => {
    const replacement = updated.get(source.id);
    return !replacement
      || Object.keys(source).length !== Object.keys(replacement).length
      || Object.entries(source).some(([key, value]) => Reflect.get(replacement, key) !== value);
  }).map((source) => source.id));
  if (changed.size === 0 && previous.length === next.length) return;
  // Additions also invalidate in-flight discovery, even without cached IDs to remove.
  invalidateSources(changed);
  try {
    localStorage.setItem(SOURCES_CHANGED_KEY, JSON.stringify({ ids: [...changed], nonce: crypto.randomUUID() }));
  } catch (error) {
    console.warn("[dynamic-discovery] source change notification failed:", error);
  }
}

export async function savePreferences(next: DiscoveryRefreshPreferences): Promise<void> {
  if (!validInterval(next.interval_seconds)) {
    throw new Error(t("dynamic_discovery.error.refresh_interval_invalid"));
  }
  if (loading) await loading;
  await invoke("set_setting", { key: SETTING_KEY, value: JSON.stringify(next) });
  preferencesRevision += 1;
  _preferences = { ...next };
  _loaded = true;
  clearTimer();
  if (running && next.auto_refresh) void refreshAutomatically();
  try {
    localStorage.setItem(PREFERENCES_CHANGED_KEY, crypto.randomUUID());
  } catch (error) {
    console.warn("[dynamic-discovery] refresh settings notification failed:", error);
  }
}

export async function startBackgroundRefresh(): Promise<void> {
  if (running && _loaded) return;
  // Keep the window's lifecycle active if reading settings fails. A later
  // settings load or save can then resume scheduling without an app reload.
  running = true;
  if (typeof window !== "undefined") window.addEventListener("storage", onStorageChanged);
  const current = runGeneration;
  await loadPreferences();
  if (running && current === runGeneration && _loaded && _preferences.auto_refresh) await refreshAutomatically();
}

export function stopBackgroundRefresh(): void {
  running = false;
  if (typeof window !== "undefined") window.removeEventListener("storage", onStorageChanged);
  runGeneration += 1;
  generation += 1;
  clearTimer();
}
