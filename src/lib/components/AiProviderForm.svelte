<script lang="ts">
  import { onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import type { AiProviderRecord, LlmProtocol } from "../ai/types.ts";
  import * as ai from "../ai/store.svelte.ts";
  import { createCodexLogin } from "../ai/codex-login.svelte.ts";
  import { writeText } from "../clipboard.ts";
  import { t, errMsg, errCoded } from "../i18n/index.svelte.ts";
  import SearchSelect from "./SearchSelect.svelte";
  import AppIcon from "./AppIcon.svelte";

  let {
    provider,
    protocolCards,
    endpointChips,
    onSave,
    onCancel,
    onChange,
  }: {
    provider: AiProviderRecord;
    protocolCards: { protocol: LlmProtocol; label: string; subKey: string }[];
    endpointChips: Record<LlmProtocol, { label: string; url: string }[]>;
    onSave: (id: string) => void;
    onCancel: () => void;
    onChange?: () => void;
  } = $props();

  let formId = $state("");
  let formProtocol = $state<LlmProtocol>("openai-completions");
  let formName = $state("");
  let formEndpoint = $state("");
  let formApiKey = $state("");
  let formHasKey = $state(false);
  let formModel = $state("");
  let saving = $state(false);
  let loadingModels = $state(false);
  let modelsGeneration = 0;
  let note = $state<string | null>(null);
  let modelOptions = $state<{ value: string; label: string }[]>([]);
  let loadedSourceId = $state<string | null>(null);
  const codex = createCodexLogin();
  const isCodex = $derived(formProtocol === "codex-subscription");
  const login = $derived(codex.challenge());
  let alive = true;
  onDestroy(() => {
    alive = false;
    codex.dispose();
    modelsGeneration += 1;
    loadingModels = false;
  });

  function loadFromSource(p: AiProviderRecord) {
    formId = p.id;
    formProtocol = p.protocol;
    formName = p.name ?? "";
    formEndpoint = p.endpoint ?? "";
    formApiKey = "";
    formHasKey = p.has_api_key;
    formModel = p.model ?? "";
    modelOptions = p.model ? [{ value: p.model, label: p.model }] : [];
    note = null;
    if (p.protocol === "codex-subscription" && p.id) void codex.load(p.id);
  }

  // New (id "") and edit (row) share one form; the parent remounts via {#key}
  // for a fresh add, so this effect only runs once per mount.
  $effect(() => {
    if (provider.id === loadedSourceId) return;
    loadedSourceId = provider.id;
    loadFromSource(provider);
  });

  const formError = $derived(
    !formName.trim()
      ? t("ai.settings.provider.error.name_required")
      : !isCodex && !formEndpoint.trim()
        ? t("ai.settings.provider.error.endpoint_required")
        : !formModel.trim()
          ? t("ai.settings.provider.error.model_required")
          : "",
  );

  function fillEndpoint(url: string) {
    formEndpoint = url;
  }

  function selectProtocol(protocol: LlmProtocol) {
    if (protocol === formProtocol) return;
    codex.dispose();
    modelsGeneration += 1;
    loadingModels = false;
    formProtocol = protocol;
    modelOptions = [];
    note = null;
    if (protocol === "codex-subscription" && provider.protocol === protocol && formId) {
      void codex.load(formId);
    }
  }

  async function signIn() {
    if (!formName.trim() || saving || codex.busy()) return;
    saving = true;
    note = null;
    try {
      // Explicitly labelled "Save and sign in": persist a provider before
      // authorizing it, then keep the form open for model selection.
      const id = await ai.saveProvider({
        id: formId || undefined,
        name: formName.trim(),
        protocol: "codex-subscription",
        model: formModel.trim(),
        endpoint: "",
      });
      onChange?.();
      if (!alive) return;
      formId = id;
      await codex.start(id);
    } catch (e) {
      if (alive) note = t("ai.settings.note.save_failed", { error: errMsg(e) });
    } finally {
      if (alive) saving = false;
    }
  }

  async function signOut() {
    modelsGeneration += 1;
    loadingModels = false;
    modelOptions = [];
    await codex.logout(formId);
    onChange?.();
  }

  async function openLoginPage() {
    if (!login) return;
    try {
      await invoke("open_external_url", { url: login.verification_url });
    } catch (e) {
      note = errMsg(e);
    }
  }

  async function copyCode() {
    if (!login) return;
    try {
      await writeText(login.user_code);
      note = t("ai.settings.codex.code_copied");
    } catch (e) {
      note = errMsg(e);
    }
  }

  let wasAuthenticated = false;
  $effect(() => {
    const authenticated = codex.authenticated();
    if (authenticated && !wasAuthenticated && isCodex) {
      void loadModels();
      onChange?.();
    }
    wasAuthenticated = authenticated;
  });

  /** 拉模型：显式按钮，失败给反馈（接口不开放的厂商就留空手填）。 */
  async function loadModels() {
    const current = ++modelsGeneration;
    loadingModels = true;
    note = null;
    try {
      const list = await ai.listModels(formProtocol, isCodex ? "" : formEndpoint.trim(), {
        providerId: formId || undefined,
        apiKey: isCodex ? undefined : formApiKey.trim() || undefined,
      });
      if (!alive || current !== modelsGeneration) return;
      modelOptions = list.map((m) => ({ value: m.id, label: m.display_name ?? m.id }));
      note = t("ai.settings.note.models_loaded", { count: list.length });
    } catch (e: any) {
      if (alive && current === modelsGeneration) {
        note = t("ai.settings.note.models_failed", { error: errMsg(e) });
        if (isCodex && formId && errCoded(e)?.code === "codex_login_required") {
          await codex.load(formId);
        }
      }
    } finally {
      if (alive && current === modelsGeneration) loadingModels = false;
    }
  }

  async function handleSave() {
    if (formError || saving || login || codex.busy()) return;
    saving = true;
    note = null;
    try {
      const id = await ai.saveProvider({
        id: formId || undefined,
        name: formName.trim(),
        protocol: formProtocol,
        model: formModel.trim(),
        endpoint: isCodex ? "" : formEndpoint.trim(),
        apiKey: isCodex ? undefined : formApiKey.trim() || undefined,
      });
      onSave(id);
    } catch (e: any) {
      note = t("ai.settings.note.save_failed", { error: errMsg(e) });
    } finally {
      saving = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") handleSave();
  }
</script>

<div class="card inline-form">
  <div class="protocol-grid" aria-label={t("ai.settings.provider.protocol")} role="group">
    {#each protocolCards as card (card.protocol)}
      <button
        type="button"
        class="protocol-card"
        class:active={formProtocol === card.protocol}
        class:p-ds={card.protocol === "deepseek-thinking"}
        class:p-oai={card.protocol === "openai-completions"}
        class:p-ant={card.protocol === "anthropic-messages"}
        aria-pressed={formProtocol === card.protocol}
        onclick={() => selectProtocol(card.protocol)}
        disabled={saving || codex.busy()}
      >
        <span class="protocol-icon"><AppIcon name="ai" size={17} /></span>
        <span class="protocol-text">
          <span class="protocol-title">{card.label}</span>
          <span class="protocol-sub">{t(card.subKey)}</span>
        </span>
      </button>
    {/each}
  </div>

  <label class="field">
    <span class="label-text">{t("common.name")}</span>
    <input type="text" bind:value={formName} placeholder={t("ai.settings.provider.name_placeholder")} onkeydown={handleKeydown} />
  </label>

  {#if isCodex}
    <div class="subscription-auth">
      <p class="auth-hint">{t("ai.settings.codex.hint")}</p>
      {#if login}
        <p class="auth-hint">{t("ai.settings.codex.instructions")}</p>
        <div class="auth-actions">
          <code class="device-code">{login.user_code}</code>
          <button type="button" class="btn btn-sm" onclick={copyCode}>{t("ai.settings.codex.copy_code")}</button>
          <button type="button" class="btn btn-accent btn-sm" onclick={openLoginPage}>{t("ai.settings.codex.open_page")}</button>
        </div>
        <span class="auth-url">{login.verification_url}</span>
        <div class="auth-actions">
          <span class="auth-hint" role="status">{t("ai.settings.codex.waiting")}</span>
          <button type="button" class="btn btn-sm" onclick={() => codex.cancel()}>{t("common.cancel")}</button>
        </div>
      {:else if codex.authenticated()}
        <div class="auth-actions">
          <span class="auth-hint" role="status">{t("ai.settings.codex.signed_in")}</span>
          <button type="button" class="btn btn-sm" onclick={signOut} disabled={codex.busy()}>{t("ai.settings.codex.sign_out")}</button>
        </div>
      {:else}
        <button type="button" class="btn btn-sm" onclick={signIn} disabled={saving || codex.busy() || !formName.trim()}>
          {saving || codex.busy() ? t("ai.settings.codex.connecting") : t("ai.settings.codex.save_sign_in")}
        </button>
      {/if}
      {#if codex.error()}<div class="form-error" role="alert">{errMsg(codex.error())}</div>{/if}
    </div>
  {:else}
  <div class="field">
    <div class="label-row">
      <span class="label-text" id={`endpoint-label-${formId || "new"}`}>{t("ai.settings.label.endpoint")}</span>
      {#each endpointChips[formProtocol] as chip (chip.url)}
        <button type="button" class="chip" onclick={() => fillEndpoint(chip.url)}>{chip.label}</button>
      {/each}
    </div>
    <input
      type="text"
      class="mono"
      bind:value={formEndpoint}
      placeholder="https://…"
      aria-labelledby={`endpoint-label-${formId || "new"}`}
      onkeydown={handleKeydown}
    />
  </div>

  <label class="field">
    <span class="label-text">{t("ai.settings.label.api_key")}</span>
    <input
      type="password"
      bind:value={formApiKey}
      placeholder={formHasKey ? t("ai.settings.placeholder.api_key_set") : t("ai.settings.placeholder.api_key_unset")}
    />
  </label>
  {/if}

  <div class="field">
    <label class="label-text" for={`ai-model-${formId || "new"}`}>{t("ai.settings.label.model")}</label>
    <div class="model-row">
      <SearchSelect
        id={`ai-model-${formId || "new"}`}
        bind:value={formModel}
        options={modelOptions}
        allowCustom
        ariaLabel={t("ai.settings.label.model")}
        placeholder={t("ai.settings.placeholder.model")}
        searchPlaceholder={t("ai.settings.placeholder.model")}
        emptyText={t("ai.settings.model.empty")}
      />
      <button type="button" class="btn btn-sm" onclick={loadModels} disabled={loadingModels || (isCodex && !codex.authenticated())}>
        {loadingModels ? t("ai.settings.btn.loading_models") : t("ai.settings.btn.load_models")}
      </button>
    </div>
  </div>

  <div class="form-actions">
    <button class="btn btn-accent btn-sm" onclick={handleSave} disabled={!!formError || saving || !!login || codex.busy()}>
      {saving ? t("ai.settings.btn.saving") : t("common.save")}
    </button>
    <button class="btn btn-sm" onclick={onCancel}>{t("common.cancel")}</button>
    {#if note}<span class="note">{note}</span>{/if}
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
  .protocol-grid {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
  }
  .protocol-card {
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
  .protocol-card:hover:not(.active) {
    background: var(--surface);
  }
  .protocol-card.active {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, var(--bg));
    color: var(--accent);
  }
  .protocol-card.p-ds.active {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, var(--bg));
    color: var(--accent);
  }
  .protocol-card.p-oai.active {
    border-color: var(--success);
    background: color-mix(in srgb, var(--success) 12%, var(--bg));
    color: var(--success);
  }
  .protocol-card.p-ant.active {
    border-color: var(--purple);
    background: color-mix(in srgb, var(--purple) 12%, var(--bg));
    color: var(--purple);
  }
  .protocol-icon {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--text-sub);
    background: var(--surface);
  }
  .protocol-card.p-ds .protocol-icon {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .protocol-card.p-oai .protocol-icon {
    color: var(--success);
    background: color-mix(in srgb, var(--success) 12%, transparent);
  }
  .protocol-card.p-ant .protocol-icon {
    color: var(--purple);
    background: color-mix(in srgb, var(--purple) 12%, transparent);
  }
  .protocol-text {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .protocol-title {
    font-size: 13px;
    font-weight: 650;
    white-space: nowrap;
  }
  .protocol-sub {
    font-size: 11px;
    color: var(--text-sub);
    white-space: nowrap;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .label-text {
    font-size: 12px;
    color: var(--text-sub);
  }
  .subscription-auth {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    min-width: 0;
  }
  .auth-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }
  .auth-hint, .auth-url {
    margin: 0;
    font-size: 12px;
    color: var(--text-sub);
    overflow-wrap: anywhere;
  }
  .device-code {
    font-size: 18px;
    letter-spacing: 0.08em;
    user-select: all;
  }
  .label-row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex-wrap: wrap;
  }
  /* Endpoint quick-fill chips — plain hyperlinks after the label, not pills. */
  .chip {
    padding: 0;
    border: none;
    background: none;
    font-family: inherit;
    font-size: 12px;
    color: var(--accent);
    cursor: pointer;
  }
  .chip:hover {
    text-decoration: underline;
  }
  .inline-form input[type="text"],
  .inline-form input[type="password"] {
    width: 100%;
    box-sizing: border-box;
  }
  .model-row {
    display: flex;
    gap: 8px;
    align-items: stretch;
  }
  .model-row :global(.search-select) {
    flex: 1;
    min-width: 0;
  }
  .model-row .btn {
    flex-shrink: 0;
  }
  .form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
  }
  .note {
    font-size: 12px;
    color: var(--accent);
  }
  .form-error {
    font-size: 12px;
    color: var(--error, #ff6b6b);
    background: rgba(255, 107, 107, 0.08);
    padding: 6px 10px;
    border-radius: 4px;
  }
  @media (max-width: 640px) {
    .protocol-grid {
      width: 100%;
    }
  }
</style>
