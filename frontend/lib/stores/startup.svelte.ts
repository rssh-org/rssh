import { invoke } from "@tauri-apps/api/core";
import * as app from "./app.svelte.ts";
import { errMsg } from "../i18n/index.svelte.ts";
import { toast } from "./toast.svelte.ts";

// Only references to saved profiles are persisted. Never serialize tab.meta:
// serial login scripts, editor content and connector commands may contain secrets.
type SavedTerminal = { type: "ssh" | "telnet" | "serial"; profileId: string } | { type: "local" };
interface Snapshot { version: 1; terminals: SavedTerminal[]; active: number }
export type StartupAction = "restore" | "local" | "home";
const actionKey = "startup_action";
const snapshotKey = "session_restore.snapshot";
let _ready = $state(false);
let writes = Promise.resolve();

export async function action(): Promise<StartupAction> {
  const value = await invoke<string | null>("get_setting", { key: actionKey });
  return value === "restore" || value === "local" ? value : "home";
}
export function ready() { return _ready; }

export async function availableShells(): Promise<string[]> {
  return app.capabilities().localPty ? invoke<string[]>("list_shells") : [];
}

function report(error: unknown) {
  console.warn("[startup]", error);
  toast.error(errMsg(error));
}

function queueWrite(operation: () => Promise<void>): Promise<void> {
  // Keep action reads and their writes together, including switching away + clear.
  const next = writes.then(operation);
  writes = next.catch((error) => console.warn("[startup] write failed:", error));
  return next;
}

export async function setAction(value: StartupAction) {
  await queueWrite(async () => {
    await invoke("set_setting", { key: actionKey, value });
    if (value !== "restore") await invoke("set_setting", { key: snapshotKey, value: "" });
  });
  if (value === "restore") await persist(snapshot());
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
  for (const tab of app.workspaceTabs()) {
    const saved = reference(tab);
    if (!saved) continue;
    if (tab.id === app.activeWorkspaceId()) active = terminals.length;
    terminals.push(saved);
  }
  return JSON.stringify({ version: 1, terminals, active } satisfies Snapshot);
}

export async function persist(value: string): Promise<void> {
  // Only a primary window whose startup reads succeeded may replace the snapshot.
  if (!_ready) return;
  await queueWrite(async () => {
    if (await action() === "restore") {
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

export async function run(signal?: AbortSignal): Promise<void> {
  let failed = false;
  try {
    const selectedAction = await action();
    if (selectedAction === "home" || signal?.aborted || app.tabs().length !== 1) return;
    if (selectedAction === "local") {
      const shells = await availableShells();
      if (shells.length && await action() === "local" && !signal?.aborted && app.tabs().length === 1) {
        app.addTab({ id: `local:${crypto.randomUUID()}`, type: "local", label: "Local" });
      }
      return;
    }
    const saved = parse(await invoke<string | null>("get_setting", { key: snapshotKey }));
    if (!saved || signal?.aborted) return;
    const types = new Set(saved.terminals.map((entry) => entry.type));
    const [ssh, telnet, serial] = await Promise.all([
      types.has("ssh") ? app.loadProfiles() : [],
      types.has("telnet") ? invoke<app.TelnetProfile[]>("list_telnet_profiles") : [],
      types.has("serial") && app.capabilities().serial ? invoke<app.SerialProfile[]>("list_serial_profiles") : [],
    ]);
    // The preference or the user's tabs may have changed while profiles loaded.
    const stillRestoring = await action() === "restore";
    if (!stillRestoring || signal?.aborted || app.tabs().length !== 1) return;
    let activeId = "home";
    const restoredIds: string[] = [];
    saved.terminals.forEach((entry, index) => {
      const before = app.tabs().length;
      if (entry.type === "local" && app.capabilities().localPty) {
        app.addTab({ id: `local:${crypto.randomUUID()}`, type: "local", label: "Local" });
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
}
