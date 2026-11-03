<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import * as app from "../stores/app.svelte.ts";
  import type {
    ConnectionKind,
    Forward,
    Group,
    Profile,
    SerialProfile,
    TelnetProfile,
  } from "../stores/app.svelte.ts";
  import { toast } from "../stores/toast.svelte.ts";
  import { errMsg, t } from "../i18n/index.svelte.ts";
  import { pickTextFile } from "../pick-file.ts";
  import { saveTextFile, fileStamp } from "../save-file.ts";
  import {
    buildConnectionItems,
    groupConnectionItems,
    type ConnectionListItem,
  } from "./connection-list.ts";
  import AppIcon from "./AppIcon.svelte";
  import { connectionIconName } from "./app-icon";

  let profiles = $state<Profile[]>([]);
  let forwards = $state<Forward[]>([]);
  let serialProfiles = $state<SerialProfile[]>([]);
  let telnetProfiles = $state<TelnetProfile[]>([]);
  let groups = $state<Group[]>([]);
  let loading = $state(true);
  let deletingKey = $state<string | null>(null);
  let importing = $state(false);
  let msg = $state("");

  let items = $derived(buildConnectionItems({ profiles, forwards, serialProfiles, telnetProfiles }));
  let sections = $derived(groupConnectionItems(groups, items, t("profile.ungrouped")));

  onMount(refresh);

  async function loadOrEmpty<T>(loader: () => Promise<T[]>): Promise<T[]> {
    try {
      return await loader();
    } catch (error) {
      toast.error(errMsg(error));
      return [];
    }
  }

  async function refresh() {
    loading = true;
    [profiles, forwards, serialProfiles, telnetProfiles, groups] = await Promise.all([
      loadOrEmpty(app.loadProfiles),
      loadOrEmpty(app.loadForwards),
      app.capabilities().serial ? loadOrEmpty(app.loadSerialProfiles) : Promise.resolve([]),
      loadOrEmpty(app.loadTelnetProfiles),
      loadOrEmpty(app.loadGroups),
    ]);
    loading = false;
  }

  function openMigrate() {
    invoke("open_external_url", { url: "https://rssh-org.github.io/migrate/" })
      .catch((e) => toast.error(errMsg(e)));
  }

  // Partial import errors list each failure and need longer to read.
  function clearMsgLater() {
    const ttl = msg.includes("\n") ? 15000 : 4000;
    setTimeout(() => (msg = ""), ttl);
  }

  async function doExport() {
    try {
      const json = await invoke<string>("export_config");
      const path = await saveTextFile(json, {
        defaultName: `rssh-config-${fileStamp()}.json`,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      msg = path ? t("import_export.exported_to", { path }) : "";
    } catch (e) { msg = `${t("toast.error.export")}: ${errMsg(e)}`; }
    clearMsgLater();
  }

  async function doImport() {
    importing = true;
    try {
      const f = await pickTextFile({ accept: ".json,application/json" });
      if (!f) {
        msg = "";
        return;
      }
      try {
        await invoke("import_config", { json: f.text });
        msg = t("import_export.imported_from", { path: f.name });
      } finally {
        // Partial imports can save entries before reporting an error.
        await refresh();
      }
    } catch (e) { msg = `${t("toast.error.import")}: ${errMsg(e)}`; }
    finally {
      importing = false;
      clearMsgLater();
    }
  }

  function itemKey(item: ConnectionListItem): string {
    return `${item.kind}:${item.id}`;
  }

  function kindLabel(kind: ConnectionKind): string {
    switch (kind) {
      case "ssh": return t("connection.type.ssh");
      case "forward": return t("connection.type.forward");
      case "serial": return t("connection.type.serial");
      case "telnet": return t("connection.type.telnet");
    }
  }

  function deleteCommand(kind: ConnectionKind): string {
    switch (kind) {
      case "ssh": return "delete_profile";
      case "forward": return "delete_forward";
      case "serial": return "delete_serial_profile";
      case "telnet": return "delete_telnet_profile";
    }
  }

  async function remove(item: ConnectionListItem) {
    const key = itemKey(item);
    deletingKey = key;
    try {
      await invoke(deleteCommand(item.kind), { id: item.id });
      await refresh();
    } catch (error) {
      toast.error(`${t("toast.error.delete")}: ${errMsg(error)}`);
    } finally {
      deletingKey = null;
    }
  }
</script>

<div class="page" aria-busy={loading}>
  <div class="toolbar">
    <button type="button" class="btn btn-sm" title={t("import_export.export_title")} onclick={doExport}>
      {t("common.export")}
    </button>
    <button type="button" class="btn btn-sm" title={t("import_export.import_title")} onclick={doImport} disabled={importing}>
      {importing ? t("common.importing") : t("common.import")}
    </button>
    <button type="button" class="btn btn-sm" onclick={openMigrate}>
      {t("import_export.migrate_title")}
    </button>
    <button type="button" class="btn btn-accent btn-sm" onclick={() => app.openConnectionCreate()}>
      {t("connection.new")}
    </button>
  </div>

  {#if msg}
    <div class="toast" role="status">{msg}</div>
  {/if}

  {#if loading}
    <p class="empty" aria-live="polite">{t("common.loading")}</p>
  {:else}
    {#each sections as section (section.key)}
      <div class="group-head">
        {#if section.color}<span class="dot" style:background={section.color} aria-hidden="true"></span>{/if}
        <span class="group-name">{section.name}</span>
        <span class="group-count">{section.items.length}</span>
      </div>
      {#each section.items as item (itemKey(item))}
        <div class="card item-row">
          <div class="item-main">
            <span class="item-icon"><AppIcon name={connectionIconName(item.kind)} size={18} /></span>
            <div class="item-info">
              <div class="item-title-row">
                <span class="item-name">{item.name}</span>
                <span class="item-kind">{kindLabel(item.kind)}</span>
              </div>
              <div class="item-sub">{item.detail}</div>
            </div>
          </div>
          <div class="item-actions">
            <button
              type="button"
              class="btn btn-sm btn-icon"
              title={t("common.copy")}
              aria-label={`${t("common.copy")} ${kindLabel(item.kind)} ${item.name}`}
              onclick={() => app.openConnectionCopy(item.kind, item.id)}
            >
              <AppIcon name="copy" size={16} />
            </button>
            <button
              type="button"
              class="btn btn-sm btn-icon"
              title={t("common.edit")}
              aria-label={`${t("common.edit")} ${kindLabel(item.kind)} ${item.name}`}
              onclick={() => app.openConnectionEdit(item.kind, item.id)}
            >
              <AppIcon name="edit" size={16} />
            </button>
            <button
              type="button"
              class="btn btn-sm btn-icon btn-danger"
              title={t("common.delete")}
              aria-label={`${t("common.delete")} ${kindLabel(item.kind)} ${item.name}`}
              aria-busy={deletingKey === itemKey(item)}
              onclick={() => remove(item)}
              disabled={deletingKey === itemKey(item)}
            >
              {#if deletingKey === itemKey(item)}…{:else}<AppIcon name="trash" size={16} />{/if}
            </button>
          </div>
        </div>
      {/each}
    {:else}
      <p class="empty">{t("connection.empty")}</p>
    {/each}
  {/if}
</div>

<style>
  .page { padding: 24px; }
  .toolbar { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: 8px; margin-bottom: 16px; }
  .toast {
    padding: 8px 14px;
    margin-bottom: 16px;
    background: var(--accent-soft);
    color: var(--accent);
    border-radius: var(--radius-sm);
    font-size: 12px;
    font-weight: 500;
    white-space: pre-line;
    overflow-wrap: anywhere;
  }
  .group-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 20px 0 10px;
    padding-bottom: 6px;
  }
  .toolbar + .group-head { margin-top: 0; }
  .dot { width: 10px; height: 10px; border-radius: 50%; flex-shrink: 0; }
  .group-name { color: var(--text); font-size: 13px; font-weight: 600; }
  .group-count {
    padding: 1px 8px;
    border-radius: 10px;
    background: var(--surface);
    color: var(--text-sub);
    font-size: 11px;
  }
  .item-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 16px;
  }
  .item-main { display: flex; align-items: center; gap: 12px; min-width: 0; }
  .item-icon {
    display: grid;
    place-items: center;
    flex: 0 0 auto;
    width: 32px;
    height: 32px;
    border-radius: 7px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 12px;
    font-weight: 700;
  }
  .item-info { min-width: 0; }
  .item-title-row { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .item-name { overflow: hidden; font-size: 14px; font-weight: 600; text-overflow: ellipsis; white-space: nowrap; }
  .item-kind {
    flex-shrink: 0;
    padding: 1px 6px;
    border: 1px solid var(--divider);
    border-radius: 999px;
    color: var(--text-sub);
    font-size: 10px;
  }
  .item-sub {
    overflow: hidden;
    color: var(--text-sub);
    font-family: var(--term-font);
    font-size: 12px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .item-actions { display: flex; flex: 0 0 auto; gap: 10px; }
  .empty { padding: 32px; color: var(--text-dim); text-align: center; }

  @media (max-width: 720px) {
    .item-row { align-items: flex-start; flex-direction: column; }
    /* Column + flex-start shrink-wraps .item-main to content width, which
       unbounds the ellipsis chain — a long forward detail then overflows the
       viewport. Stretch back to the row width so .item-sub truncates. */
    .item-main { width: 100%; }
    .item-actions { align-self: flex-end; flex-wrap: wrap; justify-content: flex-end; }
  }
</style>
