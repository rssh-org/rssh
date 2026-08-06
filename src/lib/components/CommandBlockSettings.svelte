<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import * as app from "../stores/app.svelte.ts";
  import { errMsg, t } from "../i18n/index.svelte.ts";
  import type { CommandBlockRedactRule } from "../stores/app.svelte.ts";

  let commandBlockBar = $state(true);
  let autoColorBlocks = $state(false);
  let promptEnabled = $state(true);
  let promptReplacement = $state("anonymous@rssh");
  let redactRules = $state<CommandBlockRedactRule[]>([]);
  let editingRule = $state<CommandBlockRedactRule | null>(null);
  let isNewRule = $state(false);
  let savingRule = $state(false);
  let savingPrompt = $state(false);
  let redactionLoading = $state(true);
  let redactionReady = $state(false);
  let redactNote = $state<string | null>(null);
  let confirmingRuleDelete = $state(false);
  let confirmRuleDeleteTimer: number | null = null;

  onMount(async () => {
    const [bar, autoColor] = await Promise.all([
      app.loadCommandBlockBar(),
      app.loadAutoColorBlocks(),
    ]);
    commandBlockBar = bar;
    autoColorBlocks = autoColor;
    try {
      applyRedaction(await app.loadCommandBlockRedaction(true));
      redactionReady = true;
    } catch (error) {
      redactNote = t("settings.shell.command_block_redact_error_load", { error: errMsg(error) });
    } finally {
      redactionLoading = false;
    }
  });

  onDestroy(() => {
    if (confirmRuleDeleteTimer !== null) clearTimeout(confirmRuleDeleteTimer);
  });

  function applyRedaction(redaction: app.CommandBlockRedactionSettings) {
    promptEnabled = redaction.promptEnabled;
    promptReplacement = redaction.promptReplacement;
    redactRules = redaction.rules;
  }

  async function saveCommandBlockBar() {
    await app.setCommandBlockBar(commandBlockBar);
  }

  async function saveAutoColorBlocks() {
    await app.setAutoColorBlocks(autoColorBlocks);
  }

  async function savePromptEnabled() {
    const previous = !promptEnabled;
    savingPrompt = true;
    redactNote = null;
    try {
      await app.setCommandBlockPromptRedactEnabled(promptEnabled);
    } catch (error) {
      promptEnabled = previous;
      redactNote = t("settings.shell.command_block_redact_error_save", { error: errMsg(error) });
    } finally {
      savingPrompt = false;
    }
  }

  async function savePromptReplacement() {
    savingPrompt = true;
    redactNote = null;
    try {
      await app.setCommandBlockPromptReplacement(promptReplacement);
    } catch (error) {
      redactNote = t("settings.shell.command_block_redact_error_save", { error: errMsg(error) });
    } finally {
      savingPrompt = false;
    }
  }

  function resetRuleDeleteConfirm() {
    confirmingRuleDelete = false;
    if (confirmRuleDeleteTimer !== null) {
      clearTimeout(confirmRuleDeleteTimer);
      confirmRuleDeleteTimer = null;
    }
  }

  function newRule() {
    editingRule = {
      id: "user-" + crypto.randomUUID().slice(0, 8),
      pattern: "",
      replacement: "",
    };
    isNewRule = true;
    redactNote = null;
    resetRuleDeleteConfirm();
  }

  function viewRule(rule: CommandBlockRedactRule) {
    editingRule = { ...rule };
    isNewRule = false;
    redactNote = null;
    resetRuleDeleteConfirm();
  }

  function cancelRuleEdit() {
    editingRule = null;
    isNewRule = false;
    redactNote = null;
    resetRuleDeleteConfirm();
  }

  async function saveRule() {
    if (!editingRule) return;
    if (editingRule.pattern === "" || editingRule.replacement === "") {
      redactNote = t("settings.shell.command_block_redact_error_empty");
      return;
    }
    savingRule = true;
    redactNote = null;
    try {
      await app.saveCommandBlockRedactRule(editingRule);
      editingRule = null;
      isNewRule = false;
      applyRedaction(app.commandBlockRedaction());
    } catch (error) {
      redactNote = t("settings.shell.command_block_redact_error_save", { error: errMsg(error) });
    } finally {
      savingRule = false;
    }
  }

  async function removeRule(rule: CommandBlockRedactRule) {
    if (!confirmingRuleDelete) {
      confirmingRuleDelete = true;
      confirmRuleDeleteTimer = window.setTimeout(() => {
        confirmingRuleDelete = false;
        confirmRuleDeleteTimer = null;
      }, 3000);
      return;
    }
    resetRuleDeleteConfirm();
    try {
      await app.deleteCommandBlockRedactRule(rule.id);
      editingRule = null;
      isNewRule = false;
      applyRedaction(app.commandBlockRedaction());
    } catch (error) {
      redactNote = t("settings.shell.command_block_redact_error_delete", { error: errMsg(error) });
    }
  }
</script>

<div class="page">
  <div class="section-label">{t("settings.shell.command_block")}</div>
  <!-- 命令块侧栏开关（主）→ 开启后展开：自动染色开关 + 快捷键提示。
       跟 .danger-card 同款"主开关 + 分隔 + 子内容"结构。色带渲染被侧栏开关罩着，
       关掉侧栏就没有块也没有染色，所以子项天然嵌在开启分支里。 -->
  <div class="card surface-raised cmd-block-card">
    <div class="cmd-block-head">
      <div class="cmd-block-head-body">
        <div class="cmd-block-title"
             class:on={commandBlockBar} class:off={!commandBlockBar}>
          {t("settings.shell.command_block_bar")}
        </div>
        <div class="cmd-block-desc">{t("settings.shell.command_block_bar_desc")}</div>
      </div>
      <label class="switch">
        <input type="checkbox" bind:checked={commandBlockBar} onchange={saveCommandBlockBar} />
        <span class="slider"></span>
      </label>
    </div>

    {#if commandBlockBar}
      <div class="card-divider"></div>
      <div class="cmd-block-head">
        <div class="cmd-block-head-body">
          <div class="cmd-block-title" class:on={autoColorBlocks} class:off={!autoColorBlocks}>
            {t("settings.shell.command_block_auto_color")}
          </div>
          <div class="cmd-block-desc">{t("settings.shell.command_block_auto_color_desc")}</div>
        </div>
        <label class="switch">
          <input type="checkbox" bind:checked={autoColorBlocks} onchange={saveAutoColorBlocks} />
          <span class="slider"></span>
        </label>
      </div>

      <div class="card-divider"></div>
      <div class="tips-group">
        <div class="tips-title">{t("settings.shell.command_block_tips_title")}</div>
        <ul class="tips-list">
          <li>{t("settings.shell.command_block_tip_click")}</li>
          <li>{t("settings.shell.command_block_tip_shift_click")}</li>
          <li>{t("settings.shell.command_block_tip_cmd_click")}</li>
          <li>{t("settings.shell.command_block_tip_right_click")}</li>
          <li>{t("settings.shell.command_block_tip_clear")}</li>
        </ul>
      </div>
    {/if}
  </div>

  <div class="section-label">{t("settings.shell.command_block_copy_redaction")}</div>
  <div class="card surface-raised cmd-block-card">
    <div class="cmd-block-head">
      <div class="cmd-block-head-body">
        <div id="prompt-redact-title" class="cmd-block-title" class:on={promptEnabled}>
          {t("settings.shell.command_block_prompt_redact")}
        </div>
        <div id="prompt-redact-desc" class="cmd-block-desc">
          {t("settings.shell.command_block_prompt_redact_desc")}
        </div>
      </div>
      <label class="switch">
        <input
          type="checkbox"
          bind:checked={promptEnabled}
          disabled={!redactionReady || savingPrompt}
          onchange={savePromptEnabled}
          aria-labelledby="prompt-redact-title"
          aria-describedby="prompt-redact-desc"
        />
        <span class="slider"></span>
      </label>
    </div>

    {#if promptEnabled}
      <div class="card-divider"></div>
      <div class="prompt-replacement-row">
        <div class="row prompt-replacement-field">
          <label for="prompt-replacement">
            {t("settings.shell.command_block_prompt_replacement")}
          </label>
          <input
            id="prompt-replacement"
            type="text"
            class="mono"
            bind:value={promptReplacement}
            disabled={!redactionReady}
            placeholder="anonymous@rssh"
          />
        </div>
        <button class="btn btn-accent btn-sm" onclick={savePromptReplacement} disabled={!redactionReady || savingPrompt}>
          {savingPrompt ? t("ai.settings.btn.saving") : t("common.save")}
        </button>
      </div>
    {/if}
  </div>

  <div class="card surface-raised rules-card">
    <div class="card-head">
      <span class="hint">{t("settings.shell.command_block_redact_hint")}</span>
      {#if redactionReady && !editingRule}
        <button class="btn btn-sm" onclick={newRule}>
          {t("settings.shell.command_block_redact_new")}
        </button>
      {/if}
    </div>

    {#if redactNote}
      <div class="banner">
        {redactNote}
        <button class="banner-close" onclick={() => (redactNote = null)} aria-label={t("common.close")}>×</button>
      </div>
    {/if}

    {#if !editingRule}
      <div class="rule-list">
        {#if redactionLoading}
          <div class="placeholder">{t("common.loading")}</div>
        {:else if redactionReady}
          {#each redactRules as rule (rule.id)}
            <button class="rule-item surface-raised-sm" onclick={() => viewRule(rule)}>
              <div class="rule-line">
                <code class="rule-pattern">{rule.pattern}</code>
                <span class="rule-arrow">→</span>
                <code class="rule-replacement">{rule.replacement}</code>
              </div>
            </button>
          {/each}
        {/if}
        {#if redactionReady && redactRules.length === 0}
          <div class="placeholder">{t("settings.shell.command_block_redact_empty")}</div>
        {/if}
      </div>
    {:else}
      <div class="form">
        <div class="row">
          <label for="cbrr-pattern">{t("settings.shell.command_block_redact_pattern")}</label>
          <input
            id="cbrr-pattern"
            type="text"
            class="mono"
            bind:value={editingRule.pattern}
            placeholder={t("settings.shell.command_block_redact_pattern_placeholder")}
          />
        </div>
        <div class="row">
          <label for="cbrr-replacement">{t("settings.shell.command_block_redact_replacement")}</label>
          <input
            id="cbrr-replacement"
            type="text"
            class="mono"
            bind:value={editingRule.replacement}
            placeholder={t("settings.shell.command_block_redact_replacement_placeholder")}
          />
        </div>
        <div class="actions">
          <button class="btn btn-accent btn-sm" onclick={saveRule} disabled={savingRule}>
            {savingRule ? t("ai.settings.btn.saving") : t("common.save")}
          </button>
          {#if !isNewRule}
            <button
              class="btn btn-sm btn-danger"
              class:confirming={confirmingRuleDelete}
              onclick={() => editingRule && removeRule(editingRule)}
            >
              {confirmingRuleDelete
                ? t("settings.shell.command_block_redact_delete_confirm")
                : t("settings.shell.command_block_redact_delete")}
            </button>
          {/if}
          <button class="btn btn-sm" onclick={cancelRuleEdit}>
            {t("settings.shell.command_block_redact_cancel")}
          </button>
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .page { padding: 24px; display: flex; flex-direction: column; gap: 16px; }

  /* 卡片：复用全局 .card.surface-raised，本地只加 padding + 内布局。 */
  .cmd-block-card {
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  /* 主开关行（title/desc + switch）。 */
  .cmd-block-head {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .cmd-block-head-body {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .cmd-block-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .cmd-block-title.on { color: var(--accent); }
  .cmd-block-desc {
    font-size: 11px;
    color: var(--text-dim);
    line-height: 1.5;
  }

  /* 卡片内分隔线：负边距贯穿到卡片左右边缘。 */
  .card-divider {
    height: 1px;
    background: var(--divider);
    margin: 2px -18px;
  }

  /* Tips 列表 —— 嵌在卡片内，不再有自己的 bg/border。 */
  .tips-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .tips-title {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-sub);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }
  .tips-list {
    margin: 0;
    padding-left: 18px;
    font-size: 12px;
    color: var(--text);
    line-height: 1.6;
  }
  .tips-list li {
    margin: 2px 0;
  }

  .prompt-replacement-row {
    display: flex;
    align-items: flex-end;
    gap: 10px;
  }
  .prompt-replacement-field { flex: 1; }

  .rules-card {
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .card-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }
  .hint {
    font-size: 11px;
    color: var(--text-dim);
    line-height: 1.5;
  }
  .banner {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    background: color-mix(in srgb, var(--error) 12%, var(--bg));
    color: var(--error);
    border-radius: 4px;
    font-size: 12px;
  }
  .banner-close {
    margin-left: auto;
    background: transparent;
    border: none;
    color: inherit;
    font-size: 14px;
    cursor: pointer;
  }
  .rule-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .rule-item {
    text-align: left;
    padding: 10px 14px;
    border: none;
    background: var(--bg);
    cursor: pointer;
    font-family: inherit;
    color: var(--text);
    transition: box-shadow 0.13s;
  }
  .rule-item:hover { box-shadow: var(--raised); }
  .rule-line {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
  }
  .rule-pattern,
  .rule-replacement {
    font-family: monospace;
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rule-pattern {
    color: var(--text);
    flex: 1 1 auto;
  }
  .rule-arrow {
    color: var(--text-dim);
    flex: 0 0 auto;
  }
  .rule-replacement {
    color: var(--accent);
    flex: 0 1 auto;
  }
  .placeholder {
    text-align: center;
    padding: 24px;
    color: var(--text-dim);
    font-size: 13px;
  }
  .form {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .row {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .row label {
    font-size: 12px;
    color: var(--text-sub);
  }
  .row input[type="text"] {
    width: 100%;
    box-sizing: border-box;
  }
  .row input.mono { font-family: monospace; }
  .actions {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 4px;
  }
  .btn-danger.confirming {
    animation: confirmPulse 1.2s ease-in-out infinite;
  }
  @keyframes confirmPulse {
    0%, 100% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--error) 45%, transparent); }
    50% { box-shadow: 0 0 0 6px color-mix(in srgb, var(--error) 0%, transparent); }
  }
</style>
