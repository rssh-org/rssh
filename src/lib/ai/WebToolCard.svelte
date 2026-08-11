<script lang="ts">
    import { t } from "../i18n/index.svelte.ts";
    import type { WebToolActivity, WebToolErrorCode } from "./types.ts";

    let { activity } = $props<{ activity: WebToolActivity }>();

    function formatBytes(bytes: number): string {
        if (bytes < 1024) return `${bytes} B`;
        if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
        return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
    }

    function errorText(code: WebToolErrorCode): string {
        switch (code) {
            case "invalid_input": return t("ai.web_tool.error.invalid_input");
            case "not_allowed": return t("ai.web_tool.error.not_allowed");
            case "unavailable": return t("ai.web_tool.error.unavailable");
            case "interrupted": return t("ai.web_tool.error.interrupted");
        }
    }
</script>

<div class="web-tool-card {activity.status}" role="status" aria-live="polite" aria-atomic="true">
    <div class="header">
        <span class="icon" aria-hidden="true">
            {#if activity.tool === "web_search"}
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <circle cx="11" cy="11" r="7"/>
                    <path d="m20 20-4-4"/>
                </svg>
            {:else}
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <circle cx="12" cy="12" r="9"/>
                    <path d="M3 12h18M12 3a15 15 0 0 1 0 18M12 3a15 15 0 0 0 0 18"/>
                </svg>
            {/if}
        </span>
        <span class="title">
            {t(activity.tool === "web_search" ? "ai.web_tool.search.title" : "ai.web_tool.fetch.title")}
        </span>
        <span class="status">
            {t(`ai.web_tool.status.${activity.status}`)}
        </span>
    </div>

    <div class="target" title={activity.target}>
        {activity.target || t("ai.web_tool.target_unavailable")}
    </div>

    {#if activity.status === "completed"}
        <div class="detail">
            {#if activity.tool === "web_search"}
                {t("ai.web_tool.search.completed", {
                    count: activity.result_count,
                    duration: activity.duration_ms,
                })}
            {:else}
                {t("ai.web_tool.fetch.completed", {
                    size: formatBytes(activity.source_bytes),
                    truncated: activity.truncated ? t("ai.web_tool.fetch.truncated") : "",
                })}
            {/if}
        </div>
    {:else if activity.status === "failed"}
        <div class="detail error">{errorText(activity.error_code)}</div>
    {/if}
</div>

<style>
    .web-tool-card {
        border: 1px solid var(--divider);
        border-left-width: 3px;
        border-radius: 6px;
        padding: calc(8px * var(--density)) calc(10px * var(--density));
        margin: calc(4px * var(--density)) 0;
        background: var(--bg);
        min-width: 0;
    }
    .web-tool-card.running { border-left-color: var(--warning); }
    .web-tool-card.completed { border-left-color: var(--success); }
    .web-tool-card.failed { border-left-color: var(--error); }
    .header {
        display: flex;
        align-items: center;
        gap: 6px;
        min-width: 0;
    }
    .icon {
        display: inline-flex;
        flex-shrink: 0;
        color: var(--text-dim);
    }
    .title {
        min-width: 0;
        flex: 1;
        font-size: 12px;
        font-weight: 600;
        color: var(--text);
    }
    .status {
        flex-shrink: 0;
        font-size: 10.5px;
        color: var(--text-dim);
    }
    .target {
        margin-top: 5px;
        overflow: hidden;
        display: -webkit-box;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 2;
        overflow-wrap: anywhere;
        font-family: monospace;
        font-size: 11px;
        line-height: 1.35;
        color: var(--text-dim);
    }
    .detail {
        margin-top: 5px;
        font-size: 11px;
        color: var(--text-dim);
        font-variant-numeric: tabular-nums;
    }
    .detail.error { color: var(--error); }
</style>
