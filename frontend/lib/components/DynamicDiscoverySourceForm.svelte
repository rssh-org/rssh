<script lang="ts">
  import type {
    DynamicDiscoveryContext,
    DynamicDiscoverySource,
    DynamicDiscoveryToolStatus,
    DynamicPlatform,
  } from "../stores/app.svelte.ts";
  import { t } from "../i18n/index.svelte.ts";
  import SearchSelect from "./SearchSelect.svelte";
  import Select from "./Select.svelte";
  import AppIcon from "./AppIcon.svelte";
  import { dynamicPlatformIconName } from "./app-icon";

  let {
    source,
    contexts,
    statuses,
    onRefreshContexts,
    onSave,
    onCancel,
  }: {
    source: DynamicDiscoverySource;
    contexts: DynamicDiscoveryContext[];
    statuses: Partial<Record<DynamicPlatform, DynamicDiscoveryToolStatus>>;
    onRefreshContexts: (platform: DynamicPlatform) => void | Promise<void>;
    onSave: (source: DynamicDiscoverySource) => void;
    onCancel: () => void;
  } = $props();

  let formId = $state("");
  let formPlatform = $state<DynamicPlatform>("docker");
  let formName = $state("");
  let formEnabled = $state(true);
  let formContext = $state("");
  let formNamespace = $state("");
  let formShell = $state("sh");
  let formRegion = $state("");
  let formAwsShell = $state<string | null>(null);
  let formOnlySsmAvailable = $state(false);
  let refreshingContexts = $state(false);
  let loadedSourceId = $state("");

  function loadFromSource(s: DynamicDiscoverySource) {
    formId = s.id;
    formPlatform = s.platform;
    formName = s.name ?? "";
    formEnabled = s.enabled ?? true;
    formContext = s.platform === "aws_ssm" ? s.profile : s.context;
    formShell = s.platform === "aws_ssm" ? "sh" : (s.shell || "sh");
    formNamespace = s.platform === "k8s" ? (s.namespace ?? "") : "";
    formRegion = s.platform === "aws_ssm" ? s.region : "";
    formAwsShell = s.platform === "aws_ssm" ? (s.shell ?? null) : null;
    formOnlySsmAvailable = s.platform === "aws_ssm" && s.only_ssm_available;
  }

  $effect(() => {
    if (source.id === loadedSourceId) return;
    loadedSourceId = source.id;
    loadFromSource(source);
  });

  const cliOptions: { platform: DynamicPlatform; label: string; command: string }[] = [
    { platform: "docker", label: "Docker CLI", command: "docker" },
    { platform: "k8s", label: "kubectl CLI", command: "kubectl" },
    { platform: "aws_ssm", label: "AWS SSM", command: "AWS CLI v2 + session-manager-plugin" },
  ];

  const contextSelectId = $derived(`dynamic-context-select-${formId || "new"}`);
  const contextOptions = $derived(
    contexts.filter((c) => c.platform === formPlatform),
  );
  const contextSelectOptions = $derived(
    contextOptions.map((ctx) => ({
      value: ctx.name,
      label: ctx.current ? `${ctx.name} (${t("dynamic_discovery.current")})` : ctx.name,
    })),
  );
  const formError = $derived(
    !cliReady(formPlatform)
      ? t("common.loading")
      : !cliAvailable(formPlatform)
      ? t("dynamic_discovery.error.cli_unavailable", { cli: cliCommand(formPlatform) })
      : !formName.trim()
      ? t("dynamic_discovery.error.name_required")
      : formPlatform === "aws_ssm"
        ? (!formRegion.trim() ? t("dynamic_discovery.error.region_required") : "")
        : !formContext.trim()
        ? t("dynamic_discovery.error.context_required")
        : !formShell.trim()
          ? t("dynamic_discovery.error.shell_required")
          : "",
  );

  const finalSource = $derived<DynamicDiscoverySource>(
    formPlatform === "aws_ssm"
      ? {
          id: formId,
          name: formName.trim(),
          enabled: formEnabled,
          platform: "aws_ssm",
          profile: formContext.trim(),
          region: formRegion.trim(),
          only_ssm_available: formOnlySsmAvailable,
          shell: formAwsShell,
        }
      : formPlatform === "docker"
      ? {
          id: formId,
          name: formName.trim(),
          enabled: formEnabled,
          platform: "docker",
          context: formContext.trim(),
          shell: formShell.trim(),
        }
      : {
          id: formId,
          name: formName.trim(),
          enabled: formEnabled,
          platform: "k8s",
          context: formContext.trim(),
          namespace: formNamespace.trim() || null,
          shell: formShell.trim(),
        },
  );

  $effect(() => {
    const options = contextOptions;
    if (formPlatform !== "aws_ssm" && !formContext.trim() && options.length > 0) {
      formContext = defaultContext(formPlatform);
    }
  });

  function cliAvailable(platform: DynamicPlatform): boolean {
    return statuses[platform]?.available === true;
  }

  function cliReady(platform: DynamicPlatform): boolean {
    return !!statuses[platform];
  }

  function cliCommand(platform: DynamicPlatform): string {
    return cliOptions.find((option) => option.platform === platform)!.command;
  }

  function cliStatus(platform: DynamicPlatform): string {
    const st = statuses[platform];
    if (!st) return t("common.loading");
    return st.available ? t("dynamic_discovery.cli_available") : t("dynamic_discovery.cli_missing");
  }

  function defaultContext(platform: DynamicPlatform): string {
    if (platform === "aws_ssm") return "";
    return contexts.find((c) => c.platform === platform && c.current)?.name
      ?? contexts.find((c) => c.platform === platform)?.name
      ?? "";
  }

  function defaultName(platform: DynamicPlatform): string {
    return platform === "aws_ssm" ? "AWS SSM" : platform === "docker" ? "Docker" : "K8S";
  }

  function choosePlatform(platform: DynamicPlatform) {
    if (!cliAvailable(platform) || platform === formPlatform) return;
    const previousDefaultName = defaultName(formPlatform);
    formPlatform = platform;
    formContext = defaultContext(platform);
    formNamespace = "";
    if (!formName.trim() || formName === previousDefaultName) {
      formName = defaultName(platform);
    }
  }

  async function refreshContexts() {
    if (refreshingContexts || !cliAvailable(formPlatform)) return;
    refreshingContexts = true;
    try {
      await onRefreshContexts(formPlatform);
    } finally {
      refreshingContexts = false;
    }
  }

  function handleSave() {
    if (formError) return;
    onSave(finalSource);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") handleSave();
  }
</script>

<div class="card inline-form">
  <div class="form-head">
    <div class="cli-grid" aria-label={t("dynamic_discovery.cli")} role="group">
      {#each cliOptions as cli (cli.platform)}
        {@const available = cliAvailable(cli.platform)}
        <button
          type="button"
          class="cli-card"
          class:active={formPlatform === cli.platform}
          class:k8s={cli.platform === "k8s"}
          disabled={!available}
          aria-pressed={formPlatform === cli.platform}
          onclick={() => choosePlatform(cli.platform)}
        >
          <span class="cli-icon"><AppIcon name={dynamicPlatformIconName(cli.platform)} size={17} /></span>
          <span class="cli-text">
            <span class="cli-title">{cli.label}</span>
            <span class="cli-sub">{cli.command} · {cliStatus(cli.platform)}</span>
          </span>
        </button>
      {/each}
    </div>
    <label class="switch" title={t("dynamic_discovery.enabled")}>
      <input type="checkbox" bind:checked={formEnabled} />
      <span class="slider"></span>
    </label>
  </div>

  <label class="field">
    <span class="label-text">{t("common.name")}</span>
    <input type="text" bind:value={formName} placeholder={t("dynamic_discovery.name_placeholder")} onkeydown={handleKeydown} />
  </label>

  <div class="field">
    <label class="label-text" for={contextSelectId}>{t(formPlatform === "aws_ssm" ? "dynamic_discovery.aws_profile" : "dynamic_discovery.context")}</label>
    <div class="context-row">
      <SearchSelect
        id={contextSelectId}
        bind:value={formContext}
        options={contextSelectOptions}
        allowCustom
        ariaLabel={t(formPlatform === "aws_ssm" ? "dynamic_discovery.aws_profile" : "dynamic_discovery.context")}
        placeholder={t(formPlatform === "aws_ssm" ? "dynamic_discovery.aws_profile_placeholder" : "dynamic_discovery.context_placeholder")}
        searchPlaceholder={t(formPlatform === "aws_ssm" ? "dynamic_discovery.aws_profile_placeholder" : "dynamic_discovery.context_placeholder")}
        emptyText={t(formPlatform === "aws_ssm" ? "dynamic_discovery.aws_profile_empty" : "dynamic_discovery.context_empty")}
      />
      <button
        type="button"
        class="btn btn-sm"
        onclick={refreshContexts}
        disabled={refreshingContexts || !cliAvailable(formPlatform)}
      >
        {t(formPlatform === "aws_ssm" ? "dynamic_discovery.refresh_profiles" : "dynamic_discovery.refresh_contexts")}
      </button>
    </div>
  </div>

  {#if formPlatform === "k8s"}
    <label class="field">
      <span class="label-text">{t("dynamic_discovery.namespace")}</span>
      <input type="text" bind:value={formNamespace} placeholder={t("dynamic_discovery.namespace_placeholder")} onkeydown={handleKeydown} />
    </label>
  {/if}

  {#if formPlatform === "aws_ssm"}
    <label class="field">
      <span class="label-text">{t("dynamic_discovery.aws_region")}</span>
      <input type="text" bind:value={formRegion} placeholder="us-east-1" onkeydown={handleKeydown} />
    </label>
    <div class="field">
      <label class="label-text" for={`dynamic-aws-shell-${formId}`}>{t("dynamic_discovery.aws_shell")}</label>
      <Select
        id={`dynamic-aws-shell-${formId}`}
        ariaLabel={t("dynamic_discovery.aws_shell")}
        value={formAwsShell}
        onchange={(value) => { formAwsShell = typeof value === "string" ? value : null; }}
        options={[
          { value: null, label: t("dynamic_discovery.aws_shell_auto") },
          { value: "sh", label: "POSIX (sh / bash / zsh)" },
          { value: "powershell", label: "PowerShell" },
          { value: "cmd", label: "cmd.exe" },
        ]}
      />
      <div class="field-hint">{t("dynamic_discovery.aws_shell_hint")}</div>
    </div>
    <label class="filter-row">
      <span>{t("dynamic_discovery.only_ssm_available")}</span>
      <span class="switch">
        <input type="checkbox" bind:checked={formOnlySsmAvailable} />
        <span class="slider"></span>
      </span>
    </label>
    <div class="field-hint">{t("dynamic_discovery.only_ssm_available_hint")}</div>
  {:else}
    <label class="field">
      <span class="label-text">{t("dynamic_discovery.shell")}</span>
      <input type="text" bind:value={formShell} placeholder="sh" onkeydown={handleKeydown} />
    </label>
  {/if}

  <div class="form-actions">
    <button class="btn btn-accent btn-sm" onclick={handleSave} disabled={!!formError}>{t("common.save")}</button>
    <button class="btn btn-sm" onclick={onCancel}>{t("common.cancel")}</button>
  </div>

  {#if formError}
    <div class="form-error">{formError}</div>
  {/if}
</div>

<style>
  .inline-form {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px;
    margin-bottom: 12px;
  }
  .form-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
  }
  .cli-grid {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    flex: 0 1 auto;
    width: fit-content;
    max-width: 100%;
    min-width: 0;
  }
  .cli-card {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: 0 0 auto;
    width: max-content;
    max-width: 100%;
    padding: 10px;
    border: 1px solid var(--divider);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--text);
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    transition: border-color 0.15s, background 0.15s, color 0.15s;
  }
  .cli-card:hover:not(:disabled):not(.active) {
    background: var(--surface);
  }
  .cli-card.active {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, var(--bg));
    color: var(--accent);
  }
  .cli-card.k8s.active {
    border-color: var(--purple);
    background: color-mix(in srgb, var(--purple) 12%, var(--bg));
    color: var(--purple);
  }
  .cli-card:disabled {
    cursor: not-allowed;
    opacity: 0.45;
  }
  .cli-icon {
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
  .cli-card.k8s .cli-icon {
    color: var(--purple);
    background: color-mix(in srgb, var(--purple) 15%, transparent);
  }
  .cli-text {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .cli-title {
    font-size: 13px;
    font-weight: 650;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cli-sub {
    font-size: 11px;
    color: var(--text-sub);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .field { display: flex; flex-direction: column; gap: 4px; }
  .label-text { font-size: 12px; color: var(--text-sub); }
  .filter-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; font-size: 13px; }
  .field-hint { font-size: 12px; line-height: 1.5; color: var(--text-sub); }
  .context-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .context-row :global(.search-select) {
    flex: 1;
    min-width: 0;
  }
  .context-row .btn {
    flex-shrink: 0;
  }
  .inline-form input[type="text"] {
    width: 100%;
    box-sizing: border-box;
  }
  .form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    align-items: center;
  }
  .form-error {
    font-size: 12px;
    color: var(--error, #ff6b6b);
    background: rgba(255, 107, 107, 0.08);
    padding: 6px 10px;
    border-radius: 4px;
  }
  @media (max-width: 640px) {
    .form-head {
      flex-direction: column;
    }
    .cli-grid {
      width: 100%;
    }
    .form-head .switch {
      align-self: flex-end;
    }
    .context-row {
      align-items: stretch;
      flex-direction: column;
    }
  }
</style>
