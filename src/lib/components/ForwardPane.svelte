<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { t, errMsg } from "../i18n/index.svelte.ts";
  import { toast } from "../stores/toast.svelte.ts";
  import type { ForwardRule } from "../stores/app.svelte.ts";

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
    rule: ForwardRule;
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
        try {
          await invoke("forward_stop", { activeId: id });
          if (destroyed || activeId !== id || generation !== pollGeneration) return;
          activeId = null;
          status = "error";
          errorMsg = t("error.ssh_disconnected");
        } catch (e: any) {
          if (destroyed || activeId !== id || generation !== pollGeneration) return;
          status = "error";
          errorMsg = errMsg(e);
        }
      }
    } catch (e: any) {
      if (destroyed || activeId !== id || generation !== pollGeneration) return;
      stopPolling();
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
      if (activeId) {
        await invoke("forward_stop", { activeId });
        if (destroyed || generation !== connectGeneration) return;
        activeId = null;
      }
      const startedId = await invoke<string>("forward_start", { forwardId: meta.forwardId });
      if (destroyed || generation !== connectGeneration) {
        await invoke("forward_stop", { activeId: startedId }).catch(() => {});
        return;
      }
      activeId = startedId;
      status = "active";
      await pollStats();
      if (activeId === startedId && status === "active") startPolling();
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
        const message = errMsg(e);
        errorMsg = message;
        toast.error(message);
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
  <div class="forward-content">
    <section class="card surface-raised summary-card">
      <div class="summary-head">
        <div class="header">
          <span class="type-badge">SSH</span>
          <div>
            <h3>{meta.name ?? "Port Forward"}</h3>
            <div class="summary-meta">
              <span>{t("forward.rule_count", { count: ruleCount })}</span>
              <span>{t("forward.via", { profile: meta.profileName ?? meta.host ?? "?" })}</span>
            </div>
          </div>
        </div>
        <div class="status-area" role="status" aria-live="polite">
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
      </div>

      <div class="stats summary-stats">
        <div class="stat">
          <span class="stat-label">{t("forward.active_connections")}</span>
          <span class="stat-value">{connections}</span>
        </div>
        <div class="stat"><span class="stat-label">TX</span><span class="stat-value">{formatBytes(bytesTx)}</span></div>
        <div class="stat"><span class="stat-label">RX</span><span class="stat-value">{formatBytes(bytesRx)}</span></div>
      </div>

      {#if errorMsg}<div class="error-msg" role="alert">{errorMsg}</div>{/if}

      <div class="actions">
        {#if status === "active"}
          <button class="btn-stop" onclick={stop}>{t("forward.stop")}</button>
        {:else if status === "error" || status === "stopped"}
          <button class="btn-reconnect" onclick={connect}>{t("common.reconnect")}</button>
        {/if}
      </div>
    </section>

    {#if rules.length > 0}
      <div class="rule-grid">
        {#each rules as rule (rule.index)}
          <article class="card surface-raised rule-card" class:rule-off={rule.status === "stopped"} class:rule-error={rule.status === "error" || rule.status === "stopping_error"}>
            <div class="rule-head">
              <span class="rule-type">{ruleType(rule)}</span>
              <code>{ruleLabel(rule)}</code>
              <label class="switch">
                <input
                  type="checkbox"
                  aria-label={ruleToggleLabel(rule)}
                  checked={rule.status === "active" || rule.status === "starting" || rule.status === "stopping_error"}
                  disabled={rulePending[rule.index] || rule.status === "starting"}
                  onchange={() => toggleRule(rule)}
                />
                <span class="slider"></span>
              </label>
            </div>
            <div class="rule-status" role="status" aria-live="polite">
              <span class="indicator" class:active={rule.status === "active"} class:connecting={rule.status === "starting"} class:error={rule.status === "error" || rule.status === "stopping_error"} class:stopped={rule.status === "stopped"}></span>
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
  </div>
</div>

<style>
  .forward-pane {
    height: 100%;
    overflow: auto;
    padding: 24px;
  }

  .forward-content {
    width: min(760px, 100%);
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .summary-card, .rule-card {
    padding: calc(18px * var(--density)) calc(20px * var(--density));
  }

  .summary-card {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .summary-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
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

  .summary-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 14px;
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-sub);
  }

  .status-area {
    display: flex;
    align-items: center;
    justify-content: flex-end;
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

  .rule-grid { display: grid; gap: 12px; }
  .rule-card {
    border-left: 3px solid var(--success);
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
    .summary-card, .rule-card { padding: 16px; }
    .summary-head { align-items: stretch; flex-direction: column; }
    .status-area { justify-content: flex-start; }
    .rule-stats { flex-wrap: wrap; }
    .rule-head { grid-template-columns: auto 1fr auto; }
  }

</style>
