<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import * as app from "../stores/app.svelte.ts";
  import type { Profile, Group, ForwardRule } from "../stores/app.svelte.ts";
  import { toast } from "../stores/toast.svelte.ts";
  import { t, errMsg } from "../i18n/index.svelte.ts";
  import Select from "./Select.svelte";
  import { connectionCopyName } from "./connection-editor.ts";

  let { id = null, copyFromId = null }: { id?: string | null; copyFromId?: string | null } = $props();

  let name = $state(""); let profileId = $state("");
  let rules = $state<ForwardRule[]>([newRule()]);
  let profiles = $state<Profile[]>([]);
  let groups = $state<Group[]>([]);
  let groupId = $state<string | null>(null);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let saving = $state(false);

  let profileOptions = $derived(profiles.map((p) => ({ value: p.id, label: p.name })));
  let forwardTypeOptions = $derived([
    { value: "local",   label: t("forward.type.local") },
    { value: "remote",  label: t("forward.type.remote") },
    { value: "dynamic", label: t("forward.type.dynamic") },
  ]);
  let groupOptions = $derived([
    { value: null, label: t("profile.none") },
    ...groups.map((g) => ({ value: g.id, label: g.name })),
  ]);
  let rulesValid = $derived(validateRules(rules));

  function newRule(existing: ForwardRule[] = []): ForwardRule {
    const used = new Set(existing.filter((rule) => rule.type !== "remote").map((rule) => rule.local_port));
    let localPort = 8080;
    while (used.has(localPort) && localPort < 65535) localPort++;
    return { type: "local", local_port: localPort, remote_host: "127.0.0.1", remote_port: 80 };
  }

  function validateRules(items: ForwardRule[]): boolean {
    const listeners = new Set<string>();
    for (const rule of items) {
      if (!Number.isInteger(rule.local_port) || rule.local_port < 0 || rule.local_port > 65535) return false;
      if (rule.type === "remote" && rule.local_port === 0) return false;
      if (rule.type !== "dynamic") {
        if (!rule.remote_host.trim() || !Number.isInteger(rule.remote_port) || rule.remote_port < 0 || rule.remote_port > 65535) return false;
        if (rule.type === "local" && rule.remote_port === 0) return false;
      }
      const listener = rule.type === "remote" ? `remote:${rule.remote_port}` : `local:${rule.local_port}`;
      if (!listener.endsWith(":0")) {
        if (listeners.has(listener)) return false;
        listeners.add(listener);
      }
    }
    return items.length > 0;
  }

  function removeRule(index: number) {
    if (rules.length > 1) rules.splice(index, 1);
  }

  onMount(async () => {
    try {
      [profiles, groups] = await Promise.all([app.loadProfiles(), app.loadGroups()]);
      const sourceId = id ?? copyFromId;
      if (sourceId) {
        const f = await invoke<any>("get_forward", { id: sourceId });
        name = copyFromId ? connectionCopyName(f.name) : f.name;
        rules = f.rules ?? [{ type: f.type, local_port: f.local_port, remote_host: f.remote_host, remote_port: f.remote_port }];
        profileId = f.profile_id;
        groupId = f.group_id ?? null;
      }
    } catch (error) {
      loadError = errMsg(error);
    } finally {
      loading = false;
    }
  });

  async function save() {
    if (loading || loadError || saving) return;
    saving = true;
    try {
      const forward = {
        id: id ?? crypto.randomUUID(),
        name,
        profile_id: profileId,
        group_id: groupId || null,
        rules,
      };
      if (id) await invoke("update_forward", { forward });
      else await invoke("create_forward", { forward });
      app.navigate("connections");
    } catch (e: any) { toast.error(`${t("toast.error.save")}: ${errMsg(e)}`); }
    finally { saving = false; }
  }
</script>

<div class="form" aria-busy={loading}>
    {#if loadError}
      <div class="form-error" role="alert">{loadError}</div>
    {/if}
    <label for="forward-name">{t("common.name")}</label>
    <input id="forward-name" type="text" bind:value={name} placeholder={t("forward.name_placeholder")} />
    <label for="forward-profile">{t("forward.profile")}</label>
    <Select id="forward-profile" bind:value={profileId} options={profileOptions} placeholder={t("forward.select")} />
    <div class="rules-header">
      <span class="rules-label">{t("forward.rules")}</span>
      <button type="button" class="btn btn-sm" onclick={() => rules.push(newRule(rules))}>{t("forward.rule_add")}</button>
    </div>
    {#each rules as rule, index (rule)}
      <fieldset class="rule">
        <legend>{t("forward.rule")} {index + 1}</legend>
        <div class="rule-head">
          <Select id={`forward-type-${index}`} bind:value={rule.type} options={forwardTypeOptions} />
          <button type="button" class="btn btn-sm" aria-label={t("forward.rule_remove")} onclick={() => removeRule(index)} disabled={rules.length === 1}>{t("common.delete")}</button>
        </div>
        {#if rule.type === "dynamic"}
          <div class="field"><label for={`forward-local-port-${index}`}>{t("forward.local_port_socks5")}</label><input id={`forward-local-port-${index}`} type="number" min="0" max="65535" bind:value={rule.local_port} /></div>
        {:else}
          <div class="row3">
            <div class="field"><label for={`forward-local-port-${index}`}>{t("forward.local_port")}</label><input id={`forward-local-port-${index}`} type="number" min={rule.type === "remote" ? 1 : 0} max="65535" bind:value={rule.local_port} /></div>
            <div class="field"><label for={`forward-remote-host-${index}`}>{t("forward.remote_host")}</label><input id={`forward-remote-host-${index}`} type="text" bind:value={rule.remote_host} /></div>
            <div class="field"><label for={`forward-remote-port-${index}`}>{t("forward.remote_port")}</label><input id={`forward-remote-port-${index}`} type="number" min={rule.type === "remote" ? 0 : 1} max="65535" bind:value={rule.remote_port} /></div>
          </div>
        {/if}
      </fieldset>
    {/each}
    <label for="forward-group">{t("profile.group")} {t("common.optional")}</label>
    <Select id="forward-group" bind:value={groupId} options={groupOptions} />
    <div class="form-actions">
      <button type="button" class="btn btn-accent btn-sm" onclick={save} disabled={loading || !!loadError || saving || !name || !profileId || !rulesValid}>
        {loading ? t("common.loading") : saving ? t("common.saving") : t("common.save")}
      </button>
      <button type="button" class="btn btn-sm" onclick={() => app.navigate("connections")}>{t("common.cancel")}</button>
    </div>
  </div>

<style>
  .form { display: flex; flex-direction: column; gap: 10px; }
  .row3 { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 8px; }
  .field { display: flex; flex-direction: column; gap: 4px; }
  .rules-header, .rule-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
  .rules-label { font-size: 12px; font-weight: 600; color: var(--text-sub); }
  .rule { display: flex; flex-direction: column; gap: 8px; margin: 0; padding: 10px; border: 1px solid var(--divider); border-radius: 6px; }
  .rule legend { padding: 0 4px; color: var(--text-sub); font-size: 12px; }
  .form-error {
    padding: 6px 10px;
    border-radius: 4px;
    background: color-mix(in srgb, var(--error) 8%, transparent);
    color: var(--error);
    font-size: 12px;
  }
  .form-actions { display: flex; justify-content: flex-end; gap: 10px; margin-top: 8px; }
  @media (max-width: 640px) { .row3 { grid-template-columns: 1fr; } }
</style>
