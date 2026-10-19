import { invoke } from "@tauri-apps/api/core";
import * as app from "./app.svelte.ts";
import { errMsg } from "../i18n/index.svelte.ts";
import { toast } from "./toast.svelte.ts";

// Only references to saved profiles are persisted. Never serialize tab.meta:
// serial login scripts, editor content and connector commands may contain secrets.
type SavedTerminal = { type: "ssh" | "telnet" | "serial"; profileId: string } | { type: "local" };
interface Snapshot { version: 1; terminals: SavedTerminal[]; active: number }
const key = `session_restore.${app.isMobile ? "mobile" : "desktop"}`;
let _enabled = $state(false);
let _ready = $state(false);
let writes = Promise.resolve();

export function enabled() { return _enabled; }
export function ready() { return _ready; }

function report(error: unknown) {
  console.warn("[session-restore]", error);
  toast.error(errMsg(error));
}

function write(setting: string, value: string): Promise<void> {
  // Serialize writes so an older snapshot cannot overwrite a newer one.
  const next = writes.then(() => invoke<void>("set_setting", { key: setting, value }));
  writes = next.catch((error) => console.warn("[session-restore] write failed:", error));
  return next;
}

export async function setEnabled(value: boolean) {
  await write(`${key}.enabled`, String(value));
  _enabled = value;
  if (!value) await write(`${key}.snapshot`, "");
}

function reference(tab: app.Tab): SavedTerminal | null {
  if (tab.type === "local") return app.isMobile ? null : { type: "local" };
  if (tab.type !== "ssh" && tab.type !== "telnet" && tab.type !== "serial") return null;
  if (tab.type === "serial" && app.isMobile) return null;
  const profileId = tab.type === "serial" ? tab.meta?.serialProfileId : tab.meta?.profileId;
  return profileId ? { type: tab.type, profileId } : null;
}

export function snapshot(): string {
  const terminals: SavedTerminal[] = [];
  let active = -1;
  for (const tab of app.tabs()) {
    const saved = reference(tab);
    if (!saved || terminals.length >= 100) continue;
    if (tab.id === app.activeTabId()) active = terminals.length;
    terminals.push(saved);
  }
  return JSON.stringify({ version: 1, terminals, active } satisfies Snapshot);
}

export function persist(value: string) {
  if (_ready && _enabled) void write(`${key}.snapshot`, value).catch(report);
}

/** Secondary windows read the preference but never restore or save their tabs. */
export async function initializeSecondaryWindow() {
  try {
    _enabled = await invoke<string | null>("get_setting", { key: `${key}.enabled` }) === "true";
  } catch (error) {
    report(error);
  } finally {
    _ready = true;
  }
}

function parse(raw: string | null): Snapshot | null {
  if (!raw) return null;
  const data = JSON.parse(raw);
  if (data?.version !== 1 || !Array.isArray(data.terminals)) return null;
  const terminals = data.terminals.slice(0, 100).map((value: unknown): SavedTerminal | null => {
    if (!value || typeof value !== "object") return null;
    const entry = value as Record<string, unknown>;
    if (entry.type === "local") return { type: "local" };
    if ((entry.type === "ssh" || entry.type === "telnet" || entry.type === "serial")
      && typeof entry.profileId === "string" && entry.profileId) {
      return { type: entry.type, profileId: entry.profileId };
    }
    return null;
  });
  // Reject invalid entries rather than shifting the active index.
  if (terminals.some((entry: SavedTerminal | null) => !entry)) return null;
  return { version: 1, terminals, active: Number.isInteger(data.active) ? data.active : -1 };
}

/** Returns whether a local terminal was restored (avoids startup duplication). */
export async function restore(signal?: AbortSignal): Promise<boolean> {
  let localRestored = false;
  try {
    _enabled = await invoke<string | null>("get_setting", { key: `${key}.enabled` }) === "true";
    if (!_enabled || signal?.aborted) return false;
    const saved = parse(await invoke<string | null>("get_setting", { key: `${key}.snapshot` }));
    if (!saved || signal?.aborted) return false;
    const types = new Set(saved.terminals.map((entry) => entry.type));
    const [ssh, telnet, serial] = await Promise.all([
      types.has("ssh") ? app.loadProfiles() : [],
      types.has("telnet") ? invoke<app.TelnetProfile[]>("list_telnet_profiles") : [],
      types.has("serial") && !app.isMobile ? invoke<app.SerialProfile[]>("list_serial_profiles") : [],
    ]);
    // Do not take over tabs the user opened while the backend was loading.
    if (signal?.aborted || app.tabs().length !== 1) return false;
    let activeId = "home";
    const restoredIds: string[] = [];
    saved.terminals.forEach((entry, index) => {
      const before = app.tabs().length;
      if (entry.type === "local" && !app.isMobile) {
        app.addTab({ id: `local:${crypto.randomUUID()}`, type: "local", label: "Local" });
        localRestored = true;
      } else if (entry.type === "ssh") {
        const profile = ssh.find((profile) => profile.id === entry.profileId);
        if (profile) app.addTab({
          id: `ssh:${crypto.randomUUID()}`, type: "ssh", label: profile.name,
          meta: { profileId: profile.id, host: profile.host, port: String(profile.port) },
        });
      } else if (entry.type === "telnet") {
        const profile = telnet.find((profile) => profile.id === entry.profileId);
        if (profile) app.connectTelnetProfile(profile);
      } else if (entry.type === "serial" && !app.isMobile) {
        const profile = serial.find((profile) => profile.id === entry.profileId);
        if (profile) app.connectSerialProfile(profile);
      }
      if (app.tabs().length > before) {
        restoredIds.push(app.activeTabId());
        if (index === saved.active) activeId = app.activeTabId();
      }
    });
    app.setActiveTab(activeId);
    // addTab/focus may apply MRU ordering. Restore the saved strip order last.
    restoredIds.forEach((id, index) => {
      app.moveTab(app.tabs().findIndex((tab) => tab.id === id), index + 1);
    });
  } catch (error) {
    _enabled = false; // Preserve the last snapshot when startup reads fail.
    report(error);
  } finally {
    if (!signal?.aborted) _ready = true;
  }
  return localRestored;
}
