<script lang="ts">
  /**
   * Plugin manager. ONE large stage mirrors the app window with EVERY plugin
   * area combined: the side column at its chosen edge (full height) and the
   * strip bar docked to the terminal region's top/bottom edge, plugins in
   * their real order. The stage IS the management surface: drag a plugin onto
   * another to reorder, hover a plugin for its name / enable switch /
   * uninstall ✕ — no separate list rows. Installing is per region (top row
   * in the side column, leftmost icon in the strip bar); a zip whose manifest
   * declares the other area is rejected by the backend.
   */
  import { onMount } from "svelte";
  import { t, errMsg, errCoded, type MessageKey } from "../i18n/index.svelte.ts";
  import { toast } from "../stores/toast.svelte.ts";
  import AppIcon from "../components/AppIcon.svelte";
  import * as plugins from "../plugins/store.svelte.ts";
  import type { PluginInfo, PluginArea } from "../plugins/store.svelte.ts";
  import { readThemeTokens, withThemeFragment } from "../plugins/bridge.ts";

  // Previews run no bridge — host theme tokens ride in as a URL fragment the
  // preview document applies itself, so plugin colors preview in-app too.
  const themeTokens = readThemeTokens(getComputedStyle(document.documentElement));

  let installing = $state(false);
  let fileInput = $state<HTMLInputElement | null>(null);
  /** Which region's install entry opened the picker — the backend enforces it. */
  let installArea = $state<PluginArea>("side");

  onMount(() => {
    if (!plugins.loaded()) void plugins.load().catch((e) => toast.error(errMsg(e)));
  });

  // ── Install (hidden input, pick-file idiom; zip bytes → base64 → Rust) ──

  function installInto(area: PluginArea): void {
    installArea = area;
    fileInput?.click();
  }

  function toBase64(bytes: Uint8Array): string {
    let binary = "";
    const chunk = 0x8000;
    for (let i = 0; i < bytes.length; i += chunk) {
      binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
    }
    return btoa(binary);
  }

  function areaName(a: string | number | undefined): string {
    const key = (a === "strip" ? "plugins.area.strip" : "plugins.area.side") as MessageKey;
    return t(key);
  }

  async function onFileChosen(e: Event): Promise<void> {
    const inputEl = e.currentTarget as HTMLInputElement;
    const file = inputEl.files?.[0];
    inputEl.value = ""; // allow picking the same file again after a failure
    if (!file) return;
    installing = true;
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      await plugins.install(toBase64(bytes), installArea);
      toast.success(t("plugins.install_done"));
    } catch (e) {
      // Area mismatch gets a fully-localized message (params name the areas).
      const coded = errCoded(e);
      if (coded?.code === "plugin_area_mismatch") {
        toast.error(t("plugins.area_mismatch", {
          expected: areaName(coded.params?.expected),
          actual: areaName(coded.params?.actual),
        }));
      } else {
        toast.error(`${t("plugins.install_failed")}: ${errMsg(e)}`);
      }
    } finally {
      installing = false;
    }
  }

  // ── Per-plugin actions ──────────────────────────────────────────────────

  async function onToggle(id: string, enabled: boolean): Promise<void> {
    try {
      await plugins.setEnabled(id, enabled);
    } catch (e) {
      toast.error(errMsg(e));
    }
  }

  // Tauri webview has no native confirm(); arm-then-fire on the button instead
  // (first ✕ arms it, a second click within the timeout uninstalls).
  let uninstallArmed = $state<string | null>(null);
  let disarmTimer: ReturnType<typeof setTimeout> | null = null;

  async function onUninstall(id: string, name: string): Promise<void> {
    if (uninstallArmed !== id) {
      uninstallArmed = id;
      if (disarmTimer) clearTimeout(disarmTimer);
      disarmTimer = setTimeout(() => (uninstallArmed = null), 3000);
      return;
    }
    if (disarmTimer) clearTimeout(disarmTimer);
    uninstallArmed = null;
    try {
      await plugins.uninstall(id);
      toast.success(`${t("plugins.uninstall_done")}: ${name}`);
    } catch (e) {
      toast.error(`${t("plugins.uninstall_failed")}: ${errMsg(e)}`);
    }
  }

  function move(id: string, targetId: string): void {
    plugins.movePluginTo(id, targetId).catch((e) => toast.error(errMsg(e)));
  }

  // ── Drag reorder (drop-on-target, same protocol as the tab strip) ───────

  let dragId = $state<string | null>(null);
  let overId = $state<string | null>(null);

  function onDragStart(e: DragEvent, id: string): void {
    dragId = id;
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
  }

  function onDragOver(e: DragEvent, id: string): void {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    overId = id;
  }

  function onDrop(e: DragEvent, id: string): void {
    e.preventDefault();
    if (dragId && dragId !== id) move(dragId, id);
    dragId = null;
    overId = null;
  }

  // Same fallback as tab reorder: some webviews never fire drop in-window, so
  // repeat the move here when a target was recorded and the drag wasn't cancelled.
  function onDragEnd(e: DragEvent): void {
    const cancelled = e.dataTransfer?.dropEffect === "none";
    if (!cancelled && dragId && overId && dragId !== overId) move(dragId, overId);
    dragId = null;
    overId = null;
  }

  // ── Section data ────────────────────────────────────────────────────────

  let sideInstalled = $derived(plugins.plugins().filter(p => p.area === "side"));
  let stripInstalled = $derived(plugins.plugins().filter(p => p.area === "strip"));
  /** Previews need the asset protocol AND a loaded registry root. */
  let previewOk = $derived(plugins.hostSupported() && plugins.loaded());
</script>

<div class="page">
  <div class="section-label">{t("settings.section.plugins")}</div>

  <input
    bind:this={fileInput}
    type="file"
    accept=".zip"
    style="display:none"
    onchange={onFileChosen}
  />
  {#if !plugins.hostSupported()}
    <div class="host-warning">{t("plugins.host_unavailable")}</div>
  {/if}

  <!-- ═══ Position pickers (both areas live in the single stage below) ═══ -->
  <div class="picker-row">
    <span class="section-label">{t("plugins.section.side")}</span>
    <div class="pos-picker">
      <button
        class:active={plugins.sidePosition() === "left"}
        onclick={() => plugins.setSidePosition("left")}
      >{t("plugins.position.left")}</button>
      <button
        class:active={plugins.sidePosition() === "right"}
        onclick={() => plugins.setSidePosition("right")}
      >{t("plugins.position.right")}</button>
    </div>
  </div>

  <div class="picker-row">
    <span class="section-label">{t("plugins.section.strip")}</span>
    <div class="pos-picker">
      <button
        class:active={plugins.stripPosition() === "top"}
        onclick={() => plugins.setStripPosition("top")}
      >{t("plugins.position.top")}</button>
      <button
        class:active={plugins.stripPosition() === "bottom"}
        onclick={() => plugins.setStripPosition("bottom")}
      >{t("plugins.position.bottom")}</button>
    </div>
  </div>

  <!-- ═══ The stage: one window mock with EVERY plugin area combined —
       side column at its chosen edge (full height), strip bar docked to the
       terminal region's top/bottom edge (same nesting as the real app). ═══ -->
  <div class="stage" class:dock-right={plugins.sidePosition() === "right"}>
    <aside class="region side-col">
      <button class="install-row" onclick={() => installInto("side")} disabled={installing}>
        <AppIcon name="add" size={14} />
        {installing ? t("plugins.installing") : t("plugins.install")}
      </button>
      {#if sideInstalled.length === 0}
        <div class="region-empty">{t("plugins.empty")}</div>
      {:else}
        {#each sideInstalled as p (p.id)}
          {@render cell(p, "side")}
        {/each}
      {/if}
    </aside>
    <div class="term-region" class:dock-bottom={plugins.stripPosition() === "bottom"}>
      <div class="region bar">
        <button
          class="bar-install"
          onclick={() => installInto("strip")}
          disabled={installing}
          title={t("plugins.install")}
          aria-label={t("plugins.install")}
        ><AppIcon name="add" size={14} /></button>
        {#if stripInstalled.length === 0}
          <div class="region-empty">{t("plugins.empty")}</div>
        {:else}
          {#each stripInstalled as p (p.id)}
            {@render cell(p, "strip")}
          {/each}
        {/if}
      </div>
      <div class="term">
        <div class="tline w55"></div>
        <div class="tline w80"></div>
        <div class="tline w40"></div>
        <div class="tline w70"></div>
      </div>
    </div>
  </div>
</div>

{#snippet cell(p: PluginInfo, kind: "side" | "strip")}
  <!-- One plugin cell: preview at rest, name + switch + ✕ on hover. Side
       blocks (130px cards) and strip segments (28px bar cells) share this. -->
  <div
    class={kind === "side" ? "block" : "seg"}
    class:off={!p.enabled}
    class:drag-over={overId === p.id && dragId !== p.id}
    class:dragging={dragId === p.id}
    draggable="true"
    title={t("plugins.drag_hint")}
    ondragstart={(e) => onDragStart(e, p.id)}
    ondragover={(e) => onDragOver(e, p.id)}
    ondrop={(e) => onDrop(e, p.id)}
    ondragend={(e) => onDragEnd(e)}
  >
    {#if previewOk && plugins.previewUrl(p)}
      <iframe
        class="preview-frame"
        src={withThemeFragment(plugins.previewUrl(p)!, themeTokens)}
        sandbox="allow-scripts"
        title={t("plugins.preview_of", { name: p.name })}
      ></iframe>
    {:else if kind === "side"}
      <div class="preview-none">{p.name}</div>
    {:else}
      <span class="seg-name">{p.name}</span>
    {/if}
    <div class="cell-hover">
      <span class="cell-label">{p.name}<span class="cell-ver">v{p.version}</span></span>
      <label class="switch" title={t("plugins.enabled_hint")}>
        <input
          type="checkbox"
          checked={p.enabled}
          onchange={(e) => onToggle(p.id, (e.currentTarget as HTMLInputElement).checked)}
        />
        <span class="slider"></span>
      </label>
      <button
        class="btn btn-sm btn-icon btn-danger"
        class:armed={uninstallArmed === p.id}
        onclick={() => onUninstall(p.id, p.name)}
        title={uninstallArmed === p.id ? t("plugins.uninstall_confirm") : t("plugins.uninstall")}
        aria-label={t("plugins.uninstall")}
      >{uninstallArmed === p.id ? "?" : "✕"}</button>
    </div>
  </div>
{/snippet}

<style>
  .page {
    padding: 24px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    flex: 1;
    overflow-y: auto;
    min-height: 0;
  }
  .host-warning {
    font-size: 12px;
    color: var(--warning);
  }

  /* ── Picker rows: area label + dock-edge segmented control ── */
  .picker-row {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .picker-row .section-label {
    padding: 0;
  }
  .pos-picker {
    display: inline-flex;
    width: fit-content;
    border: 1px solid var(--divider);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }
  .pos-picker button {
    padding: 5px 16px;
    font-size: 12px;
    font-family: inherit;
    color: var(--text-sub);
    background: transparent;
    border: none;
    cursor: pointer;
  }
  .pos-picker button + button {
    border-left: 1px solid var(--divider);
  }
  .pos-picker button.active {
    background: color-mix(in srgb, var(--accent) 15%, transparent);
    color: var(--text);
    font-weight: 600;
  }

  /* ── The stage: one big window mock = terminal + every plugin area ── */
  .stage {
    display: flex;
    height: 460px;
    border: 1px solid var(--divider);
    border-radius: var(--radius-sm);
    background: var(--bg);
    overflow: hidden;
  }
  .stage.dock-right {
    flex-direction: row-reverse;
  }
  /* Terminal region = everything beside the side column: strip bar docked
     to its top/bottom edge (column-reverse flips it), terminal below/above. */
  .term-region {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .term-region.dock-bottom {
    flex-direction: column-reverse;
  }
  .region {
    background: color-mix(in srgb, var(--accent) 5%, var(--bg));
  }
  .region.side-col {
    width: 320px;
    flex: 0 0 auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 10px;
    overflow-y: auto;
    border-right: 1px solid var(--divider);
  }
  .stage.dock-right .region.side-col {
    border-right: none;
    border-left: 1px solid var(--divider);
  }
  .region.bar {
    height: 28px; /* real strip height — the stage shows the final effect */
    flex: 0 0 auto;
    display: flex;
    flex-direction: row;
    overflow-x: auto;
    border-bottom: 1px solid var(--divider);
  }
  .term-region.dock-bottom .region.bar {
    border-bottom: none;
    border-top: 1px solid var(--divider);
  }
  .region-empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 12px;
    color: var(--text-dim);
  }

  /* Terminal placeholder — the context the plugins dock against. */
  .term {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 10px;
    padding: 24px;
  }
  .tline {
    height: 8px;
    border-radius: 4px;
    background: var(--text-dim);
    opacity: 0.3;
  }
  .w40 { width: 40%; }
  .w55 { width: 55%; }
  .w70 { width: 70%; }
  .w80 { width: 80%; }

  /* ── Install entries ── */
  .install-row {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 7px 8px;
    font-size: 12px;
    font-family: inherit;
    color: var(--text-sub);
    background: transparent;
    border: 1px dashed var(--divider);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .install-row:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--accent);
  }
  .bar-install {
    flex: 0 0 auto;
    width: 30px;
    height: 100%;
    display: grid;
    place-items: center;
    color: var(--text-sub);
    background: transparent;
    border: none;
    border-right: 1px solid var(--divider);
    cursor: pointer;
  }
  .bar-install:hover:not(:disabled) {
    color: var(--text);
    background: var(--surface);
  }
  .install-row:disabled,
  .bar-install:disabled {
    opacity: 0.5;
    cursor: default;
  }

  /* ── Plugin cells: preview at rest, hover overlay with name/switch/✕ ── */
  .block {
    position: relative;
    flex: 0 0 auto;
    height: 130px;
    border: 1px solid var(--divider);
    border-radius: var(--radius-sm);
    background: var(--surface);
    overflow: hidden;
    cursor: grab;
  }
  .block:active {
    cursor: grabbing;
  }
  .seg {
    position: relative;
    flex: 1 1 0;
    min-width: 170px;
    height: 100%;
    cursor: grab;
  }
  .seg + .seg {
    border-left: 1px solid var(--divider);
  }
  .block.dragging,
  .seg.dragging {
    opacity: 0.5;
  }
  .block.drag-over,
  .seg.drag-over {
    outline: 2px dashed var(--accent);
    outline-offset: -2px;
  }
  .block.off .preview-frame,
  .block.off .preview-none,
  .seg.off .preview-frame,
  .seg.off .seg-name {
    opacity: 0.45;
  }
  .preview-frame {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    border: none;
    display: block;
    /* Static previews: no interaction — and drags starting over the iframe
       must still reach the block/segment beneath. */
    pointer-events: none;
  }
  .preview-none {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 12px;
    color: var(--text-dim);
  }
  .seg-name {
    display: block;
    padding: 0 8px;
    line-height: 27px;
    font-size: 11px;
    color: var(--text-sub);
    white-space: nowrap;
    overflow: hidden;
  }
  .cell-hover {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 0 8px;
    background: color-mix(in srgb, var(--bg) 80%, transparent);
    opacity: 0;
    transition: opacity 0.12s;
  }
  .block:hover .cell-hover,
  .seg:hover .cell-hover {
    opacity: 1;
  }
  .cell-label {
    min-width: 0;
    font-size: 12px;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cell-ver {
    margin-left: 6px;
    font-size: 11px;
    color: var(--text-dim);
  }
  .btn.armed {
    box-shadow: var(--pressed);
    outline: 1px solid var(--error);
  }
</style>
