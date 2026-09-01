<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import * as app from "../stores/app.svelte.ts";
  import type { Credential } from "../stores/app.svelte.ts";
  import { toast } from "../stores/toast.svelte.ts";
  import { t, errMsg } from "../i18n/index.svelte.ts";
  import AppIcon from "./AppIcon.svelte";

  let items = $state<Credential[]>([]);
  onMount(async () => { items = await app.loadCredentials(); });
  let deleting = $state<string | null>(null);
  async function remove(id: string) {
    deleting = id;
    try {
      await invoke("delete_credential", { id });
      items = await app.loadCredentials();
    } catch (e: any) { toast.error(`${t("toast.error.delete")}: ${errMsg(e)}`); }
    finally { deleting = null; }
  }
</script>

<div class="page">
  <div class="toolbar">
    <button class="btn btn-accent btn-sm" onclick={() => app.navigate("credential-edit")}>{t("credential.new")}</button>
  </div>
  {#each items as c (c.id)}
    <div class="card item-row">
      <div class="item-info">
        <div class="item-name">{c.name}</div>
        <div class="item-sub">{c.username} · {c.type}</div>
      </div>
      <div class="item-actions">
        <button
          class="btn btn-sm btn-icon"
          title={t("common.edit")}
          aria-label={`${t("common.edit")} ${c.name}`}
          onclick={() => app.navigate("credential-edit", c.id)}
        >
          <AppIcon name="edit" size={16} />
        </button>
        <button
          class="btn btn-sm btn-icon btn-danger"
          title={t("common.delete")}
          aria-label={`${t("common.delete")} ${c.name}`}
          onclick={() => remove(c.id)}
          disabled={deleting === c.id}
        >
          {#if deleting === c.id}…{:else}<AppIcon name="trash" size={16} />{/if}
        </button>
      </div>
    </div>
  {:else}
    <p class="empty">{t("credential.empty")}</p>
  {/each}
</div>

<style>
  .page { padding: 24px; }
  .toolbar { display: flex; justify-content: flex-end; margin-bottom: 16px; }
  .item-row { display: flex; justify-content: space-between; align-items: center; gap: 12px; margin-bottom: 16px; }
  /* min-width + ellipsis chain: a long "user · key" sub line truncates
     instead of pushing the action buttons off screen. */
  .item-info { min-width: 0; }
  .item-name { font-weight: 600; font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .item-sub { font-size: 12px; color: var(--text-sub); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .item-actions { display: flex; gap: 10px; flex: 0 0 auto; }
  .empty { text-align: center; color: var(--text-dim); padding: 32px; }
</style>
