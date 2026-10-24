import { invoke } from "@tauri-apps/api/core";
import * as app from "./app.svelte.ts";
import { errMsg } from "../i18n/index.svelte.ts";
import { toast } from "./toast.svelte.ts";

// Only references to saved profiles are persisted. Never serialize tab.meta:
// serial login scripts, editor content and connector commands may contain secrets.
type SavedTerminal = { type: "ssh" | "telnet" | "serial"; profileId: string } | { type: "local" };
interface Snapshot { version: 1; terminals: SavedTerminal[]; active: number }
const enabledKey = "session_restore.enabled";
const snapshotKey = "session_restore.snapshot";
let _ready = $state(false);
let writes = Promise.resolve();

export async function enabled(): Promise<boolean> {
  return await invoke<string | null>("get_setting", { key: enabledKey }) === "true";
}
export function ready() { return _ready; }

function report(error: unknown) {
  console.warn("[session-restore]", error);
  toast.error(errMsg(error));
}

function queueWrite(operation: () => Promise<void>): Promise<void> {
  // Keep preference reads and their writes together, including disable + clear.
  const next = writes.then(operation);
  writes = next.catch((error) => console.warn("[session-restore] write failed:", error));
  return next;
}

export async function setEnabled(value: boolean) {
  await queueWrite(async () => {
    await invoke("set_setting", { key: enabledKey, value: String(value) });
    if (!value) await invoke("set_setting", { key: snapshotKey, value: "" });
  });
  if (value) await persist(snapshot());
}

function reference(tab: app.Tab): SavedTerminal | null {
  if (tab.type === "local") return { type: "local" };
  if (tab.type !== "ssh" && tab.type !== "telnet" && tab.type !== "serial") return null;
  const profileId = tab.type === "serial" ? tab.meta?.serialProfileId : tab.meta?.profileId;
  return profileId ? { type: tab.type, profileId } : null;
}

export function snapshot(): string {
  const terminals: SavedTerminal[] = [];
  let active = -1;
  for (const tab of app.tabs()) {
    const saved = reference(tab);
    if (!saved) continue;
    if (tab.id === app.activeTabId()) active = terminals.length;
    terminals.push(saved);
  }
  return JSON.stringify({ version: 1, terminals, active } satisfies Snapshot);
}

export async function persist(value: string): Promise<void> {
  // Only a primary window whose startup reads succeeded may replace the snapshot.
  if (!_ready) return;
  await queueWrite(async () => {
    if (await enabled()) {
      await invoke("set_setting", { key: snapshotKey, value });
    }
  }).catch(report);
}

function parse(raw: string | null): Snapshot | null {
  if (!raw) return null;
  const data = JSON.parse(raw);
  if (data?.version !== 1 || !Array.isArray(data.terminals)) return null;
  const terminals = data.terminals.map((value: unknown): SavedTerminal | null => {
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
  let failed = false;
  try {
    if (!await enabled() || signal?.aborted) return false;
    const saved = parse(await invoke<string | null>("get_setting", { key: snapshotKey }));
    if (!saved || signal?.aborted) return false;
    const types = new Set(saved.terminals.map((entry) => entry.type));
    const [ssh, telnet, serial] = await Promise.all([
      types.has("ssh") ? app.loadProfiles() : [],
      types.has("telnet") ? invoke<app.TelnetProfile[]>("list_telnet_profiles") : [],
      types.has("serial") && app.capabilities().serial ? invoke<app.SerialProfile[]>("list_serial_profiles") : [],
    ]);
    // The preference or the user's tabs may have changed while profiles loaded.
    const stillEnabled = await enabled();
    if (!stillEnabled || signal?.aborted || app.tabs().length !== 1) return false;
    let activeId = "home";
    const restoredIds: string[] = [];
    saved.terminals.forEach((entry, index) => {
      const before = app.tabs().length;
      if (entry.type === "local" && app.capabilities().localPty) {
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
      } else if (entry.type === "serial" && app.capabilities().serial) {
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
    failed = true; // Preserve the last snapshot when startup reads fail.
    report(error);
  } finally {
    _ready = !failed && !signal?.aborted;
  }
  return localRestored;
}
