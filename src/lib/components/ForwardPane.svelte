<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { t, errMsg } from "../i18n/index.svelte.ts";

  let { tabId, meta = {} }: {
    tabId: string;
    meta: Record<string, string>;
  } = $props();

  let status = $state<"connecting" | "active" | "error" | "stopped">("connecting");
  let activeId = $state<string | null>(null);
  let errorMsg = $state("");
  let bytesTx = $state(0);
  let bytesRx = $state(0);
  let connections = $state(0);
  type RuleStatus = "starting" | "active" | "stopped" | "error" | "stopping_error";
  type RuleStats = {
    index: number;
    rule: { type: "local" | "remote" | "dynamic"; local_port: number; remote_host: string; remote_port: number };
    status: RuleStatus;
    bytes_tx: number;
    bytes_rx: number;
    connections: number;
    effective_port: number | null;
    error: string | null;
  };
  type ForwardStats = { bytes_tx: number; bytes_rx: number; connections: number; connected: boolean; rules: RuleStats[] };
  let rules = $state<RuleStats[]>([]);
  let rulePending = $state<Record<number, boolean>>({});
  let pollGeneration = 0;
  let pollTimer = 0;
  let connectGeneration = 0;
  let stopGeneration = 0;
  let destroyed = false;

  function formatBytes(b: number): string {
    if (b < 1024) return `${b} B`;
    if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
    if (b < 1024 * 1024 * 1024) return `${(b / 1024 / 1024).toFixed(1)} MB`;
    return `${(b / 1024 / 1024 / 1024).toFixed(2)} GB`;
  }

  async function pollStats() {
    if (!activeId || status !== "active") return;
    const id = activeId;
    const generation = ++pollGeneration;
    try {
      const s = await invoke<ForwardStats>(
        "forward_stats", { activeId: id }
      );
      if (destroyed || activeId !== id || status !== "active" || generation !== pollGeneration) return;
      bytesTx = s.bytes_tx;
      bytesRx = s.bytes_rx;
      connections = s.connections;
      rules = s.rules;
      errorMsg = "";
      if (!s.connected) {
        stopPolling();
        await invoke("forward_stop", { activeId: id }).catch(() => {});
        if (destroyed || activeId !== id || generation !== pollGeneration) return;
        activeId = null;
        status = "error";
        errorMsg = t("error.ssh_disconnected");
      }
    } catch (e: any) {
      if (destroyed || activeId !== id || generation !== pollGeneration) return;
      stopPolling();
      activeId = null;
      status = "error";
      errorMsg = errMsg(e);
    }
  }

  function startPolling() {
    stopPolling();
    pollTimer = window.setInterval(pollStats, 2000);
  }

  function stopPolling() {
    if (pollTimer) { clearInterval(pollTimer); pollTimer = 0; }
  }

  onMount(connect);

  async function connect() {
    const generation = ++connectGeneration;
    status = "connecting";
    errorMsg = "";
    bytesTx = 0; bytesRx = 0; connections = 0;
    try {
      const startedId = await invoke<string>("forward_start", { forwardId: meta.forwardId });
      if (destroyed || generation !== connectGeneration) {
        await invoke("forward_stop", { activeId: startedId }).catch(() => {});
        return;
      }
      activeId = startedId;
      status = "active";
      await pollStats();
      startPolling();
    } catch (e: any) {
      if (destroyed || generation !== connectGeneration) return;
      status = "error";
      errorMsg = errMsg(e);
    }
  }

  async function toggleRule(rule: RuleStats) {
    if (!activeId || rulePending[rule.index]) return;
    const id = activeId;
    const start = rule.status === "stopped" || rule.status === "error";
    rulePending[rule.index] = true;
    if (start) {
      rules = rules.map((item) => item.index === rule.index
        ? { ...item, status: "starting", error: null }
        : item);
    }
    try {
      await invoke(start ? "forward_rule_start" : "forward_rule_stop", {
        activeId: id,
        ruleIndex: rule.index,
      });
      await pollStats();
      errorMsg = "";
    } catch (e: any) {
      if (!destroyed && activeId === id) {
        errorMsg = errMsg(e);
        await pollStats();
      }
    } finally {
      rulePending[rule.index] = false;
    }
  }

  function ruleLabel(rule: RuleStats): string {
    const port = rule.effective_port ?? (rule.rule.type === "remote" ? rule.rule.remote_port : rule.rule.local_port);
    if (rule.rule.type === "dynamic") return `SOCKS5 127.0.0.1:${port}`;
    if (rule.rule.type === "remote") return `remote:${port} → ${rule.rule.remote_host}:${rule.rule.local_port}`;
    return `127.0.0.1:${port} → ${rule.rule.remote_host}:${rule.rule.remote_port}`;
  }

  function ruleType(rule: RuleStats): string {
    return rule.rule.type === "local" ? "-L" : rule.rule.type === "remote" ? "-R" : "-D";
  }

  function ruleToggleLabel(rule: RuleStats): string {
    const action = rule.status === "stopped" || rule.status === "error"
      ? t("forward.start")
      : t("forward.stop");
    return `${action}: ${ruleLabel(rule)}`;
  }

  async function stop() {
    if (!activeId) return;
    const generation = ++stopGeneration;
    const id = activeId;
    status = "connecting";
    stopPolling();
    try {
      await invoke("forward_stop", { activeId: id });
      if (destroyed || generation !== stopGeneration) return;
      status = "stopped";
      if (activeId === id) activeId = null;
    } catch (e: any) {
      if (destroyed || generation !== stopGeneration) return;
      errorMsg = errMsg(e);
      status = "active";
      startPolling();
    }
  }

  onDestroy(() => {
    destroyed = true;
    connectGeneration++;
    stopGeneration++;
    pollGeneration++;
    stopPolling();
    const id = activeId;
    activeId = null;
    if (id) invoke("forward_stop", { activeId: id }).catch(() => {});
  });

  const ruleCount = $derived(rules.length || Number(meta.ruleCount ?? "1"));
</script>

<div class="forward-pane">
  <div class="card surface-raised">
    <div class="header">
      <span class="type-badge">SSH</span>
      <h3>{meta.name ?? "Port Forward"}</h3>
    </div>

    <div class="summary-line">
      <span>{t("forward.rule_count", { count: ruleCount })}</span>
      <span>{t("forward.via", { profile: meta.profileName ?? meta.host ?? "?" })}</span>
    </div>

    <div class="status-area">
      {#if status === "connecting"}
        <span class="indicator connecting"></span> <span class="status-text">{t("common.connecting")}</span>
      {:else if status === "active"}
        <span class="indicator active"></span> <span class="status-text">{t("forward.status_active")}</span>
      {:else if status === "error"}
        <span class="indicator error"></span> <span class="status-text">{t("forward.status_error")}</span>
      {:else}
        <span class="indicator stopped"></span> <span class="status-text">{t("forward.status_stopped")}</span>
      {/if}
    </div>

    {#if status === "active"}
      <div class="stats">
        <div class="stat">
          <span class="stat-label">{t("forward.active_connections")}</span>
          <span class="stat-value">{connections}</span>
        </div>
        <div class="stat">
          <span class="stat-label">TX</span>
          <span class="stat-value">{formatBytes(bytesTx)}</span>
        </div>
        <div class="stat">
          <span class="stat-label">RX</span>
          <span class="stat-value">{formatBytes(bytesRx)}</span>
        </div>
      </div>

      <div class="rule-list">
        {#each rules as rule (rule.index)}
          <article class="rule-card" class:rule-off={rule.status === "stopped"} class:rule-error={rule.status === "error" || rule.status === "stopping_error"}>
            <div class="rule-head">
              <span class="rule-type">{ruleType(rule)}</span>
              <code>{ruleLabel(rule)}</code>
              <button
                type="button"
                class="rule-toggle"
                class:on={rule.status === "active" || rule.status === "starting"}
                role="switch"
                aria-checked={rule.status === "active" || rule.status === "starting"}
                aria-label={ruleToggleLabel(rule)}
                disabled={rulePending[rule.index] || rule.status === "starting"}
                onclick={() => toggleRule(rule)}
              ><span></span></button>
            </div>
            <div class="rule-status" role="status" aria-live="polite">
              <span class="indicator" class:active={rule.status === "active"} class:connecting={rule.status === "starting"} class:error={rule.status === "error"} class:stopped={rule.status === "stopped"}></span>
              <span>{t(`forward.status_${rule.status}`)}</span>
              {#if rule.error}<span class="rule-error-text">{errMsg(rule.error)}</span>{/if}
            </div>
            <div class="rule-stats">
              <span>{t("forward.active_connections")} <b>{rule.connections}</b></span>
              <span>TX <b>{formatBytes(rule.bytes_tx)}</b></span>
              <span>RX <b>{formatBytes(rule.bytes_rx)}</b></span>
            </div>
          </article>
        {/each}
      </div>
    {/if}

    {#if errorMsg}
      <div class="error-msg" role="alert">{errorMsg}</div>
    {/if}

    <div class="actions">
      {#if status === "active"}
        <button class="btn-stop" onclick={stop}>{t("forward.stop")}</button>
      {:else if status === "error" || status === "stopped"}
        <button class="btn-reconnect" onclick={connect}>{t("common.reconnect")}</button>
      {/if}
    </div>
  </div>
</div>

<style>
  .forward-pane {
    height: 100%;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding: 24px;
  }

  .card {
    background: var(--surface);
    border: 1px solid var(--divider);
    border-radius: 12px;
    padding: calc(28px * var(--density)) calc(32px * var(--density));
    width: min(760px, 100%);
    max-height: 100%;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: calc(12px * var(--density));
  }

  .header {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .header h3 {
    font-size: 16px;
    font-weight: 600;
    color: var(--text);
    margin: 0;
  }

  .type-badge {
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    padding: 2px 8px;
    border-radius: 4px;
    background: var(--accent-soft);
    color: var(--accent);
    flex-shrink: 0;
  }

  .summary-line {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 12px;
    color: var(--text-sub);
    border-bottom: 1px solid var(--divider);
    padding-bottom: 10px;
  }

  .status-area {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 4px 0;
  }

  .indicator {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }

  .indicator.connecting { background: var(--accent); animation: pulse 1.2s infinite; }
  .indicator.active { background: var(--success); }
  .indicator.error { background: var(--error); }
  .indicator.stopped { background: var(--text-dim); }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.3; }
  }

  .status-text {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-sub);
  }

  .stats {
    display: flex;
    gap: 4px;
  }

  .stat {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    padding: 8px 4px;
    background: var(--bg);
    border-radius: 6px;
  }

  .stat-label {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-dim);
  }

  .stat-value {
    font-family: monospace;
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }

  .rule-list { display: grid; gap: 10px; }
  .rule-card {
    padding: 12px;
    border: 1px solid var(--divider);
    border-left: 3px solid var(--success);
    border-radius: 7px;
    background: var(--bg);
  }
  .rule-card.rule-off { border-left-color: var(--text-dim); opacity: 0.78; }
  .rule-card.rule-error { border-left-color: var(--error); }
  .rule-head { display: grid; grid-template-columns: auto 1fr auto; align-items: center; gap: 10px; }
  .rule-head code { color: var(--text); overflow-wrap: anywhere; }
  .rule-type {
    font: 700 11px monospace;
    color: var(--accent);
    background: var(--accent-soft);
    border-radius: 3px;
    padding: 2px 5px;
  }
  .rule-toggle {
    width: 44px;
    height: 28px;
    padding: 5px;
    border: 0;
    border-radius: 999px;
    background: var(--text-dim);
    cursor: pointer;
  }
  .rule-toggle span { display: block; width: 14px; height: 14px; border-radius: 50%; background: var(--bg); transition: transform 150ms ease; }
  .rule-toggle.on { background: var(--success); }
  .rule-toggle.on span { transform: translateX(16px); }
  .rule-toggle:disabled { cursor: wait; opacity: 0.65; }
  .rule-status { display: flex; align-items: center; gap: 6px; margin-top: 9px; font-size: 11px; color: var(--text-sub); }
  .rule-error-text { color: var(--error); margin-left: auto; }
  .rule-stats { display: flex; gap: 18px; margin-top: 8px; font: 10px monospace; color: var(--text-dim); }
  .rule-stats b { color: var(--text); font-weight: 600; }

  .error-msg {
    font-size: 12px;
    color: var(--error);
    word-break: break-all;
    text-align: center;
  }

  .actions {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding-top: 4px;
  }

  .btn-stop, .btn-reconnect {
    padding: 6px 20px;
    border-radius: 6px;
    border: none;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
  }

  .btn-stop {
    background: color-mix(in srgb, var(--error) 15%, transparent);
    color: var(--error);
  }

  .btn-stop:hover {
    background: color-mix(in srgb, var(--error) 25%, transparent);
  }

  .btn-reconnect {
    background: var(--accent);
    color: var(--bg);
  }

  @media (max-width: 600px) {
    .forward-pane { padding: 12px; }
    .card { padding: 16px; }
    .summary-line, .rule-stats { flex-wrap: wrap; }
    .rule-head { grid-template-columns: auto 1fr auto; }
  }

</style>
