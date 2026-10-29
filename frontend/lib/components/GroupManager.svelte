<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import type { Group } from "../stores/app.svelte.ts";
  import * as app from "../stores/app.svelte.ts";
  import { toast } from "../stores/toast.svelte.ts";
  import { t, errMsg } from "../i18n/index.svelte.ts";
  import AppIcon from "./AppIcon.svelte";
  import ColorInput from "./ColorInput.svelte";
  import { normalizeHexColor } from "./color-input.ts";
  import EnvironmentWave from "./EnvironmentWave.svelte";

  const uid = $props.id();

  let groups = $state<Group[]>([]);
  let adding = $state(false);
  let editId = $state<string | null>(null);

  // Form fields
  let formName = $state("");
  let formColor = $state("#4A6CF7");
  let formColorValid = $derived(normalizeHexColor(formColor) !== null);
  let formOrder = $state(0);
  let formWavesEnabled = $state(false);

  let deleting = $state<string | null>(null);

  onMount(refresh);

  async function refresh() {
    groups = await app.loadGroups();
  }

  function startAdd() {
    adding = true;
    editId = null;
    formName = "";
    formColor = "#4A6CF7";
    formOrder = 0;
    formWavesEnabled = false;
  }

  function startEdit(g: Group) {
    adding = false;
    editId = g.id;
    formName = g.name;
    formColor = g.color;
    formOrder = g.sort_order;
    formWavesEnabled = g.terminal_waves_enabled;
  }

  function cancelForm() {
    adding = false;
    editId = null;
  }

  async function saveNew() {
    if (!formName.trim() || !formColorValid) return;
    const group: Group = {
      id: crypto.randomUUID(),
      name: formName.trim(),
      color: normalizeHexColor(formColor)!,
      sort_order: formOrder,
      terminal_waves_enabled: formWavesEnabled,
    };
    try {
      await invoke("create_group", { group });
      adding = false;
      await refresh();
    } catch (e: any) { toast.error(`${t("toast.error.add")}: ${errMsg(e)}`); }
  }

  async function saveEdit() {
    if (!editId || !formName.trim() || !formColorValid) return;
    const group: Group = {
      id: editId,
      name: formName.trim(),
      color: normalizeHexColor(formColor)!,
      sort_order: formOrder,
      terminal_waves_enabled: formWavesEnabled,
    };
    try {
      await invoke("update_group", { group });
      editId = null;
      await refresh();
    } catch (e: any) { toast.error(`${t("toast.error.save")}: ${errMsg(e)}`); }
  }

  async function remove(id: string) {
    deleting = id;
    try {
      await invoke("delete_group", { id });
      if (editId === id) editId = null;
      await refresh();
    } catch (e: any) { toast.error(`${t("toast.error.delete")}: ${errMsg(e)}`); }
    finally { deleting = null; }
  }
</script>

{#snippet groupSettings()}
  <div class="group-settings">
    <div class="setting-row background-row">
      <div class="setting-body">
        <label for={`${uid}-waves`} class="setting-title" class:on={formWavesEnabled}>{t("group.terminal_background")}</label>
        <div id={`${uid}-waves-hint`} class="setting-desc">{t("group.terminal_background_hint")}</div>
      </div>
      <label class="switch">
        <input id={`${uid}-waves`} type="checkbox" role="switch" bind:checked={formWavesEnabled} aria-describedby={`${uid}-waves-hint`} />
        <span class="slider"></span>
      </label>
      {#if formWavesEnabled}
        <EnvironmentWave color={normalizeHexColor(formColor) ?? "transparent"} />
      {/if}
    </div>
    <div class="card-divider"></div>
    <div class="setting-row">
      <div class="setting-body">
        <label for={`${uid}-order`} class="setting-title">{t("group.sort_order")}</label>
      </div>
      <input id={`${uid}-order`} type="number" bind:value={formOrder} min="0" />
    </div>
    <div class="card-divider"></div>
  </div>
{/snippet}

<div class="page">
  <div class="toolbar">
    <button class="btn btn-accent btn-sm" onclick={startAdd}>{t("group.new")}</button>
  </div>

  {#if adding}
    <div class="card inline-form">
      <label for={`${uid}-name`}>{t("common.name")}</label>
      <input id={`${uid}-name`} type="text" bind:value={formName} placeholder={t("group.name_placeholder")} />
      <ColorInput bind:value={formColor} />
      {@render groupSettings()}
      <div class="form-actions">
        <button class="btn btn-accent btn-sm" onclick={saveNew} disabled={!formName.trim() || !formColorValid}>{t("common.save")}</button>
        <button class="btn btn-sm" onclick={cancelForm}>{t("common.cancel")}</button>
      </div>
    </div>
  {/if}

  {#each groups as g (g.id)}
    {#if editId === g.id}
      <div class="card inline-form">
        <label for={`${uid}-name`}>{t("common.name")}</label>
        <input id={`${uid}-name`} type="text" bind:value={formName} placeholder={t("group.name_placeholder")} />
        <ColorInput bind:value={formColor} />
        {@render groupSettings()}
        <div class="form-actions">
          <button class="btn btn-accent btn-sm" onclick={saveEdit} disabled={!formName.trim() || !formColorValid}>{t("common.save")}</button>
          <button class="btn btn-sm" onclick={cancelForm}>{t("common.cancel")}</button>
        </div>
      </div>
    {:else}
      <div class="card item-row">
        <div class="item-info">
          <span class="color-swatch" style="background: {g.color}"></span>
          <div class="item-text">
            <div class="item-name" title={g.name}>{g.name}</div>
            <div class="item-sub">{t("group.order", { n: g.sort_order })}</div>
          </div>
        </div>
        <div class="item-actions">
          <button
            class="btn btn-sm btn-icon"
            title={t("common.edit")}
            aria-label={`${t("common.edit")} ${g.name}`}
            onclick={() => startEdit(g)}
          >
            <AppIcon name="edit" size={16} />
          </button>
          <button
            class="btn btn-sm btn-icon btn-danger"
            title={t("common.delete")}
            aria-label={`${t("common.delete")} ${g.name}`}
            aria-busy={deleting === g.id}
            onclick={() => remove(g.id)}
            disabled={deleting === g.id}
          >
            {#if deleting === g.id}…{:else}<AppIcon name="trash" size={16} />{/if}
          </button>
        </div>
      </div>
    {/if}
  {:else}
    {#if !adding}
      <p class="empty">{t("group.empty")}</p>
    {/if}
  {/each}
</div>

<style>
  .page { padding: 24px; }
  .group-settings { margin: 0 -14px; }
  .setting-row { display: flex; align-items: center; gap: 12px; padding: 14px; }
  .setting-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; }
  .setting-title { font-size: 13px; font-weight: 600; color: var(--text); text-transform: uppercase; letter-spacing: 0.04em; }
  .setting-title.on { color: var(--accent); }
  .setting-desc { font-size: 11px; color: var(--text-dim); line-height: 1.5; }
  .background-row {
    position: relative;
    overflow: hidden;
    isolation: isolate;
  }
  .background-row .setting-body, .background-row .switch { position: relative; z-index: 1; }
  .switch input:focus-visible + .slider { outline: 2px solid var(--accent); outline-offset: 3px; }
  .card-divider { height: 1px; background: var(--divider); }
  .toolbar { display: flex; justify-content: flex-end; margin-bottom: 16px; }
  .item-row { display: flex; justify-content: space-between; align-items: center; gap: 12px; margin-bottom: 16px; }
  .item-info { display: flex; align-items: center; gap: 10px; min-width: 0; }
  /* min-width:0 closes the ellipsis chain across the flex wrapper — without
     it the wrapper's min-width:auto blocks truncation on .item-name. */
  .item-text { min-width: 0; }
  .item-name { font-weight: 600; font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .item-sub { font-size: 12px; color: var(--text-sub); }
  .item-actions { display: flex; gap: 10px; flex: 0 0 auto; }
  .color-swatch {
    width: 20px; height: 20px; border-radius: 4px; flex-shrink: 0;
    border: 1px solid var(--divider);
  }
  .inline-form {
    display: flex; flex-direction: column; gap: 8px;
    padding: 14px; margin-bottom: 10px;
  }
  .inline-form input[type="number"] { width: 80px; flex-shrink: 0; }
  .form-actions { display: flex; gap: 10px; margin-top: 4px; }
  .empty { text-align: center; color: var(--text-dim); padding: 32px; }
</style>
