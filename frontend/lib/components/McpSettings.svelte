<script lang="ts">
  import { onMount } from "svelte";
  import * as mcp from "../mcp/store.svelte.ts";
  import { t, errMsg } from "../i18n/index.svelte.ts";
  import { writeText } from "../clipboard.ts";

  let busy = $state(false);
  let error = $state("");
  let copiedEndpoint = $state<mcp.Endpoint | null>(null);
  const copied = $derived(copiedEndpoint !== null
    && copiedEndpoint.url === mcp.endpoint()?.url
    && copiedEndpoint.token === mcp.endpoint()?.token);

  onMount(() => { void act(mcp.refresh); });

  async function act(action: () => Promise<unknown>) {
    if (busy) return;
    busy = true;
    error = "";
    try { await action(); }
    catch (e) { error = errMsg(e); }
    finally { busy = false; }
  }

  async function copyConnection() {
    const endpoint = mcp.endpoint();
    if (!endpoint) return;
    await writeText(JSON.stringify({ url: endpoint.url, headers: { Authorization: `Bearer ${endpoint.token}` } }, null, 2));
    copiedEndpoint = endpoint;
  }
</script>

<div class="connection-head">
  <div class="connection-body">
    <div class="connection-title">{t(mcp.endpoint() ? "mcp.running" : "mcp.stopped")}</div>
    <div class="hint">{t("mcp.description")}</div>
  </div>
  <button class="btn btn-sm" disabled={busy} onclick={() => act(mcp.endpoint() ? mcp.stop : mcp.start)}>
    {t(mcp.endpoint() ? "mcp.stop" : "mcp.start")}
  </button>
</div>

{#if mcp.endpoint()}
  <div class="row">
    <label for="mcp-url">{t("mcp.url")}</label>
    <input id="mcp-url" type="text" readonly value={mcp.endpoint()!.url} />
  </div>
  <div class="row">
    <label for="mcp-token">{t("mcp.token")}</label>
    <input id="mcp-token" type="password" readonly value={mcp.endpoint()!.token} />
  </div>
  <div class="hint">{t("mcp.connection_hint")}</div>
  <div class="actions">
    <button class="btn btn-sm" disabled={busy} onclick={() => act(copyConnection)}>{t(copied ? "about.copied" : "mcp.copy")}</button>
  </div>
{/if}

{#if error || mcp.error()}<div class="error" role="alert">{error || mcp.error()}</div>{/if}

<style>
  .connection-head { display: flex; align-items: center; gap: 12px; }
  .connection-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; }
  .connection-title { font-size: 13px; font-weight: 600; color: var(--text); text-transform: uppercase; letter-spacing: 0.04em; }
  .connection-head .btn { flex-shrink: 0; }
  .hint { font-size: 11px; color: var(--text-dim); line-height: 1.5; }
  .row { display: flex; flex-direction: column; gap: 4px; }
  .row label { font-size: 12px; color: var(--text-sub); }
  .row input { width: 100%; min-width: 0; box-sizing: border-box; }
  .actions { display: flex; gap: 8px; align-items: center; margin-top: 4px; }
  .error { color: var(--error); font-size: 12px; overflow-wrap: anywhere; }
</style>
