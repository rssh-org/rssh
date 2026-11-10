<script lang="ts">
  import { onMount } from "svelte";
  import type {
    DynamicDiscoveryContext,
    DynamicDiscoverySource,
    DynamicDiscoveryToolStatus,
    DynamicPlatform,
  } from "../stores/app.svelte.ts";
  import * as app from "../stores/app.svelte.ts";
  import * as discovery from "../stores/discovery.svelte.ts";
  import { toast } from "../stores/toast.svelte.ts";
  import { t, errMsg } from "../i18n/index.svelte.ts";
  import DynamicDiscoverySourceForm from "./DynamicDiscoverySourceForm.svelte";
  import AppIcon from "./AppIcon.svelte";
  import { dynamicPlatformIconName } from "./app-icon";

  let items = $state<DynamicDiscoverySource[]>([]);
  let contexts = $state<DynamicDiscoveryContext[]>([]);
  let statuses = $state<Partial<Record<DynamicPlatform, DynamicDiscoveryToolStatus>>>({});
  let adding = $state<DynamicDiscoverySource | null>(null);
  let addKey = $state(0);
  let editId = $state<string | null>(null);
  let savingSources = $state(false);
  let savingRefresh = $state(false);
  let refreshSettingsError = $state("");

  onMount(() => {
    void refresh();
    void refreshPlatformInfo();
    void discovery.loadPreferences().catch((error) => {
      refreshSettingsError = errMsg(error);
    });
  });

  async function refresh() {
    try {
      items = await app.loadDynamicDiscoverySources();
    } catch (e: any) {
      items = [];
      toast.error(errMsg(e));
    }
  }

  async function loadToolStatus(platform: DynamicPlatform): Promise<DynamicDiscoveryToolStatus> {
    try {
      return await app.dynamicDiscoveryToolStatus(platform);
    } catch (e: any) {
      toast.error(errMsg(e));
      return {
        platform,
        available: false,
        version: null,
        error: errMsg(e),
      };
    }
  }

  async function refreshPlatformInfo() {
    await Promise.all((["docker", "k8s", "aws_ssm"] as const).map(refreshContextInfo));
  }

  async function refreshContextInfo(platform: DynamicPlatform) {
    const [status, nextContexts] = await Promise.all([
      loadToolStatus(platform),
      app.loadDynamicDiscoveryContexts(platform).catch(() => []),
    ]);
    statuses = { ...statuses, [platform]: status };
    contexts = [
      ...contexts.filter((c) => c.platform !== platform),
      ...nextContexts,
    ];
  }

  function defaultContext(platform: DynamicPlatform): string {
    return contexts.find((c) => c.platform === platform && c.current)?.name
      ?? contexts.find((c) => c.platform === platform)?.name
      ?? "";
  }

  function defaultPlatform(): DynamicPlatform {
    if (statuses.docker?.available) return "docker";
    if (statuses.k8s?.available) return "k8s";
    if (statuses.aws_ssm?.available) return "aws_ssm";
    return "docker";
  }

  function blankSource(platform: DynamicPlatform): DynamicDiscoverySource {
    if (platform === "aws_ssm") return {
      id: crypto.randomUUID(), name: "AWS SSM", enabled: true,
      platform: "aws_ssm", profile: "", region: "", only_ssm_available: false,
    };
    return platform === "docker"
      ? {
          id: crypto.randomUUID(),
          name: "Docker",
          enabled: true,
          platform: "docker",
          context: defaultContext("docker"),
          shell: "sh",
        }
      : {
          id: crypto.randomUUID(),
          name: "K8S",
          enabled: true,
          platform: "k8s",
          context: defaultContext("k8s"),
          namespace: null,
          shell: "sh",
        };
  }

  function startAdd() {
    editId = null;
    addKey += 1;
    adding = blankSource(defaultPlatform());
  }

  function startEdit(source: DynamicDiscoverySource) {
    adding = null;
    editId = source.id;
  }

  function cancelForm() {
    adding = null;
    editId = null;
  }

  async function saveAll(next: DynamicDiscoverySource[]): Promise<boolean> {
    if (savingSources) return false;
    savingSources = true;
    const previous = items;
    try {
      await app.saveDynamicDiscoverySources(next);
      discovery.sourcesChanged(previous, next);
      items = next;
      await refresh();
      return true;
    } catch (e: any) {
      toast.error(`${t("toast.error.save")}: ${errMsg(e)}`);
      return false;
    } finally {
      savingSources = false;
    }
  }

  async function saveNew(source: DynamicDiscoverySource) {
    if (await saveAll([...items, source])) adding = null;
  }

  async function saveEdit(source: DynamicDiscoverySource) {
    if (await saveAll(items.map((it) => it.id === source.id ? source : it))) editId = null;
  }

  async function toggleEnabled(source: DynamicDiscoverySource, event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const next = items.map((it) => it.id === source.id ? { ...it, enabled: input.checked } : it);
    await saveAll(next);
    input.checked = items.find((it) => it.id === source.id)?.enabled ?? false;
  }

  async function remove(id: string) {
    await saveAll(items.filter((it) => it.id !== id));
    if (editId === id) editId = null;
  }

  async function manualRefresh() {
    try {
      const snapshot = await discovery.refresh();
      if (snapshot.errors.length) {
        toast.error(t("dynamic_discovery.probe_partial", { targets: snapshot.targets.length, errors: snapshot.errors.length }));
      } else {
        toast.success(t("dynamic_discovery.probe_ok", { targets: snapshot.targets.length }));
      }
    } catch (e: any) {
      toast.error(errMsg(e));
    }
  }

  async function saveRefreshPreferences(next: discovery.DiscoveryRefreshPreferences) {
    if (savingRefresh) return;
    savingRefresh = true;
    try {
      await discovery.savePreferences(next);
      refreshSettingsError = "";
    } catch (error) {
      toast.error(errMsg(error));
    } finally {
      savingRefresh = false;
    }
  }

  async function changeInterval(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    await saveRefreshPreferences({ ...discovery.preferences(), interval_seconds: Number(input.value) });
    input.value = String(discovery.preferences().interval_seconds);
  }

  async function changeAutoRefresh(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    await saveRefreshPreferences({ ...discovery.preferences(), auto_refresh: input.checked });
    input.checked = discovery.preferences().auto_refresh;
  }

  function sourceMeta(source: DynamicDiscoverySource): string {
    if (source.platform === "docker") return `Docker · ${source.context}`;
    if (source.platform === "aws_ssm") return `AWS SSM · ${source.profile || t("dynamic_discovery.aws_default_profile")} · ${source.region}`;
    return `K8S · ${source.context}${source.namespace ? ` · ${source.namespace}` : ` · ${t("dynamic_discovery.all_namespaces")}`}`;
  }
</script>

<div class="page">
  <div class="toolbar">
    <button class="btn btn-accent btn-sm" onclick={startAdd} disabled={savingSources}>{t("dynamic_discovery.new")}</button>
  </div>

  <section class="card refresh-settings" aria-labelledby="discovery-refresh-title">
    <div class="refresh-heading">
      <h2 id="discovery-refresh-title">{t("dynamic_discovery.refresh_title")}</h2>
      <button class="btn btn-sm" onclick={manualRefresh} disabled={discovery.refreshing()}>
        {t(discovery.refreshing() ? "dynamic_discovery.refreshing" : "dynamic_discovery.manual_refresh")}
      </button>
    </div>
    <p class="refresh-hint">{t("dynamic_discovery.refresh_hint")}</p>
    <div class="refresh-controls">
      <label class="refresh-toggle">
        <span>{t("dynamic_discovery.auto_refresh")}</span>
        <span class="switch">
          <input type="checkbox" checked={discovery.preferences().auto_refresh}
            disabled={!discovery.loaded() || savingRefresh}
            onchange={changeAutoRefresh} />
          <span class="slider"></span>
        </span>
      </label>
      <label class="refresh-interval">
        <span>{t("dynamic_discovery.refresh_interval")}</span>
        <input type="number" min="1" max="86400" step="1" value={discovery.preferences().interval_seconds}
          disabled={!discovery.loaded() || savingRefresh} onchange={changeInterval} />
      </label>
    </div>
    {#if refreshSettingsError || discovery.failure()}
      <p class="refresh-error" role="alert">{refreshSettingsError || errMsg(discovery.failure())}</p>
    {/if}
    {#each discovery.errors() as error}
      <p class="refresh-error" role="alert">{error.source_name}: {errMsg(error.message)}</p>
    {/each}
  </section>

  {#if adding}
    {#key addKey}
      <fieldset class="source-form" disabled={savingSources}>
        <DynamicDiscoverySourceForm
          source={adding}
          {contexts}
          {statuses}
          onRefreshContexts={refreshContextInfo}
          onSave={saveNew}
          onCancel={cancelForm}
        />
      </fieldset>
    {/key}
  {/if}

  {#each items as source (source.id)}
    {#if editId === source.id}
      <fieldset class="source-form" disabled={savingSources}>
        <DynamicDiscoverySourceForm
          {source}
          {contexts}
          {statuses}
          onRefreshContexts={refreshContextInfo}
          onSave={saveEdit}
          onCancel={cancelForm}
        />
      </fieldset>
    {:else}
      <div class="card item-row">
        <div class="item-info" class:dimmed={!source.enabled}>
          <div class="item-icon" class:k8s={source.platform === "k8s"}>
            <AppIcon name={dynamicPlatformIconName(source.platform)} size={17} />
          </div>
          <div class="item-text">
            <div class="item-name">{source.name}</div>
            <div class="item-sub">{sourceMeta(source)}</div>
          </div>
        </div>
        <div class="item-actions">
          <label class="switch" title={t("dynamic_discovery.enabled")}>
            <input type="checkbox" checked={source.enabled} disabled={savingSources} onchange={(event) => toggleEnabled(source, event)} />
            <span class="slider"></span>
          </label>
          <button
            class="btn btn-sm btn-icon"
            title={t("common.edit")}
            aria-label={`${t("common.edit")} ${source.name}`}
            onclick={() => startEdit(source)}
            disabled={savingSources}
          >
            <AppIcon name="edit" size={16} />
          </button>
          <button
            class="btn btn-sm btn-icon btn-danger"
            title={t("common.delete")}
            aria-label={`${t("common.delete")} ${source.name}`}
            onclick={() => remove(source.id)}
            disabled={savingSources}
          >
            <AppIcon name="trash" size={16} />
          </button>
        </div>
      </div>
    {/if}
  {:else}
    {#if !adding}
      <p class="empty">{t("dynamic_discovery.empty")}</p>
    {/if}
  {/each}
</div>

<style>
  .page { padding: 24px; min-height: 0; }
  .source-form { border: 0; margin: 0; padding: 0; min-inline-size: 0; }
  .refresh-settings { margin-bottom: 16px; }
  .refresh-heading, .refresh-controls, .refresh-toggle, .refresh-interval { display: flex; align-items: center; gap: 12px; }
  .refresh-heading { justify-content: space-between; }
  .refresh-heading h2 { font-size: 14px; font-weight: 600; margin: 0; }
  .refresh-hint { font-size: 12px; color: var(--text-sub); margin: 10px 0 16px; }
  .refresh-controls { flex-wrap: wrap; gap: 16px 24px; }
  .refresh-toggle, .refresh-interval { font-size: 13px; }
  .refresh-interval input { width: 100px; }
  .refresh-error { color: var(--error); font-size: 12px; overflow-wrap: anywhere; margin: 10px 0 0; }
  .toolbar {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: 8px;
    margin-bottom: 16px;
  }
  .item-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 16px;
    gap: 12px;
  }
  .item-info {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    flex: 0 1 auto;
    width: fit-content;
  }
  .item-info.dimmed { opacity: 0.45; }
  .item-icon {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--accent);
    background: var(--accent-soft);
  }
  .item-icon.k8s {
    color: var(--purple);
    background: color-mix(in srgb, var(--purple) 15%, transparent);
  }
  .item-text { min-width: 0; }
  .item-name {
    font-weight: 600;
    font-size: 14px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .item-sub {
    font-size: 12px;
    color: var(--text-sub);
    font-family: var(--term-font);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .item-actions { display: flex; align-items: center; gap: 10px; flex-shrink: 0; }
  .empty { text-align: center; color: var(--text-dim); padding: 32px; }
  @media (max-width: 640px) {
    .toolbar { justify-content: stretch; flex-wrap: wrap; }
    .item-row { align-items: flex-start; flex-direction: column; }
    /* Column + flex-start shrink-wraps .item-info (base width: fit-content),
       unbounding the ellipsis chain — a long context/namespace line then
       overflows the viewport. Stretch back to the row width so it truncates. */
    .item-info { width: 100%; }
    .item-actions { align-self: flex-end; }
  }
</style>
