/**
 * Plugin registry + per-tab panel state. Mirrors the ai store's idioms:
 * open-state is per tab (kept alive across tab switches), area positions and
 * the strip height persist to localStorage like `ai_panel_position`.
 *
 * Lazy contract: nothing here mounts an iframe. A plugin's UI loads only for
 * tabs where the user opened the plugin panel (see PluginSide/PluginStrip),
 * and exec channels are one-shot on the Rust side.
 */

import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { createSidePanelState } from "../stores/panel-state.svelte.ts";

export type PluginArea = "side" | "strip";
export type SidePosition = "left" | "right";
export type StripPosition = "top" | "bottom";

export interface PluginInfo {
  id: string;
  name: string;
  version: string;
  description: string;
  author: string;
  area: PluginArea;
  preview: string;
  enabled: boolean;
  installed_at: number;
  sort_order: number;
}

// ── Positions (localStorage, same pattern as ai_panel_position) ──

const SIDE_POS_KEY = "plugin_side_position";
const STRIP_POS_KEY = "plugin_strip_position";

function loadSidePos(): SidePosition {
  const v = localStorage.getItem(SIDE_POS_KEY);
  return v === "left" || v === "right" ? v : "right";
}

function loadStripPos(): StripPosition {
  const v = localStorage.getItem(STRIP_POS_KEY);
  return v === "top" || v === "bottom" ? v : "bottom";
}

// ── State ────────────────────────────────────────────────────────────────

let _plugins = $state<PluginInfo[]>([]);
let _loaded = $state(false);
let _pluginsRoot = $state<string | null>(null);
let _sidePos = $state<SidePosition>(loadSidePos());
let _stripPos = $state<StripPosition>(loadStripPos());
/** Per-tab panel state — the shared side-panel skeleton, in-memory only. */
const panel = createSidePanelState({ minWidth: 280 });

// ── Registry ─────────────────────────────────────────────────────────────

export function plugins() { return _plugins; }
export function loaded() { return _loaded; }

/** Load registry + on-disk root once (AppShell onMount / manager page open). */
export async function load(): Promise<void> {
  const [list, root] = await Promise.all([
    invoke<PluginInfo[]>("list_plugins"),
    invoke<string>("plugins_root"),
  ]);
  _plugins = list;
  _pluginsRoot = root;
  _loaded = true;
}

/** Install into a specific region: the backend rejects a manifest whose
 *  area doesn't match (side plugins can't land in the strip and vice versa). */
export async function install(base64Zip: string, area: PluginArea): Promise<void> {
  await invoke("install_plugin", { base64Zip, area });
  await load();
}

export async function setEnabled(id: string, enabled: boolean): Promise<void> {
  // Optimistic toggle: the manager page checkbox should not lag on IPC.
  _plugins = _plugins.map(p => (p.id === id ? { ...p, enabled } : p));
  try {
    await invoke("set_plugin_enabled", { id, enabled });
  } catch (e) {
    _plugins = _plugins.map(p => (p.id === id ? { ...p, enabled: !enabled } : p));
    throw e;
  }
}

export async function uninstall(id: string): Promise<void> {
  await invoke("uninstall_plugin", { id });
  _plugins = _plugins.filter(p => p.id !== id);
}

export function sidePlugins() {
  return _plugins.filter(p => p.enabled && p.area === "side");
}

export function stripPlugins() {
  return _plugins.filter(p => p.enabled && p.area === "strip");
}

/** Asset-protocol URL for a plugin document (entry or declared preview);
 *  null when the path is absent or the root is not loaded yet. */
function packageUrl(plugin: PluginInfo, doc: string): string | null {
  if (!_pluginsRoot || !doc) return null;
  return convertFileSrc(`${_pluginsRoot}/${plugin.id}/${doc}`);
}

export function entryUrl(plugin: PluginInfo): string | null {
  return packageUrl(plugin, "index.html");
}

export function previewUrl(plugin: PluginInfo): string | null {
  return packageUrl(plugin, plugin.preview);
}

/** Drop-on-target move within one area (same semantics as tab reorder: the
 *  dragged plugin takes the target's slot). Persists the full new sequence. */
export async function movePluginTo(id: string, targetId: string): Promise<void> {
  if (id === targetId) return;
  const plugin = _plugins.find(p => p.id === id);
  const target = _plugins.find(p => p.id === targetId);
  if (!plugin || !target || plugin.area !== target.area) return;
  const areaIds = _plugins.filter(p => p.area === plugin.area).map(p => p.id);
  const from = areaIds.indexOf(id);
  const to = areaIds.indexOf(targetId);
  if (from < 0 || to < 0) return;
  areaIds.splice(from, 1);
  areaIds.splice(to, 0, id);
  await invoke("set_plugin_order", { ids: areaIds });
  await load();
}

/**
 * Whether this host can serve plugin files at all. In the real Tauri webview
 * convertFileSrc maps to the asset protocol; the JCEF/browser shim returns
 * the path unchanged (no scheme), where an iframe could never load it.
 */
export function hostSupported(): boolean {
  return /^(asset|https?):/i.test(convertFileSrc("/probe"));
}

// ── Per-tab panel state (kept alive across tab switches) ─────────────────
// Thin named wrappers over the shared skeleton — call sites stay unchanged.

export function isOpen(tabId: string): boolean { return panel.isOpen(tabId); }
export function openPanel(tabId: string): void { panel.openPanel(tabId); }
export function closePanel(tabId: string): void { panel.closePanel(tabId); }
export function togglePanel(tabId: string): void { panel.togglePanel(tabId); }

/** Called from the app store's tab-close cleanup (same spot as SFTP state). */
export function disposeTab(tabId: string): void {
  panel.clearTab(tabId);
}

export function sideWidth(tabId: string): number | null { return panel.width(tabId); }
export function setSideWidth(tabId: string, width: number | null): void {
  panel.setWidth(tabId, width);
}

// ── Area positions & strip height ────────────────────────────────────────

export function sidePosition(): SidePosition { return _sidePos; }
export function setSidePosition(pos: SidePosition): void {
  _sidePos = pos;
  localStorage.setItem(SIDE_POS_KEY, pos);
}

export function stripPosition(): StripPosition { return _stripPos; }
export function setStripPosition(pos: StripPosition): void {
  _stripPos = pos;
  localStorage.setItem(STRIP_POS_KEY, pos);
}
