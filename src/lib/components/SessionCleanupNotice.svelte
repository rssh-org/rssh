<script lang="ts">
    import { failedSessionCleanups, retrySessionCleanup } from "../stores/session-cleanup.svelte.ts";
    import { errMsg, t } from "../i18n/index.svelte.ts";
</script>

{#if failedSessionCleanups().length > 0}
    <aside class="cleanup-notices" aria-label={t("session.cleanup_failed")}>
        {#each failedSessionCleanups() as item (item.sessionId)}
            <div class="cleanup-notice surface-raised">
                <div role="alert">
                    <strong>{t("session.cleanup_failed")}: {item.label}</strong>
                    <p>{errMsg(item.error)}</p>
                </div>
                <button class="btn" onclick={() => void retrySessionCleanup(item.sessionId)}>{t("common.retry")}</button>
            </div>
        {/each}
    </aside>
{/if}

<style>
    .cleanup-notices {
        position: fixed;
        bottom: calc(16px + env(safe-area-inset-bottom, 0px));
        right: calc(16px + env(safe-area-inset-right, 0px));
        z-index: 1000;
        display: flex;
        flex-direction: column;
        gap: 8px;
        width: min(420px, calc(100% - 32px));
        max-height: 50%;
        overflow-y: auto;
    }
    .cleanup-notice {
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 12px;
        border-left: 3px solid var(--error);
        border-radius: var(--radius-sm);
        background: var(--bg);
        color: var(--text);
        font-size: 13px;
    }
    .cleanup-notice > div { flex: 1; min-width: 0; overflow-wrap: anywhere; }
    .cleanup-notice p { margin: 4px 0 0; color: var(--text-sub); }
    .cleanup-notice button { flex-shrink: 0; }
</style>
