<script lang="ts">
    import { toasts, dismiss } from "../stores/toast.svelte.ts";
    import { failedSessionCleanups, retrySessionCleanup } from "../stores/session-cleanup.svelte.ts";
    import { errMsg, t } from "../i18n/index.svelte.ts";
</script>

<div class="toast-stack">
    {#each toasts() as item (item.id)}
        <button class="toast toast-{item.kind} surface-raised" onclick={() => dismiss(item.id)}>
            {item.message}
        </button>
    {/each}
    {#each failedSessionCleanups() as item (item.sessionId)}
        <div class="toast toast-error cleanup-notice surface-raised">
            <div role="alert">
                <strong>{t("session.cleanup_failed")}: {item.label}</strong>
                <p>{errMsg(item.error)}</p>
            </div>
            <button class="btn" onclick={() => void retrySessionCleanup(item.sessionId)}>{t("common.retry")}</button>
        </div>
    {/each}
</div>

<style>
    .toast-stack {
        position: fixed;
        top: calc(16px + env(safe-area-inset-top, 0px));
        right: calc(16px + env(safe-area-inset-right, 0px));
        z-index: 1000;
        display: flex;
        flex-direction: column;
        gap: 8px;
        max-width: min(420px, calc(100vw - 32px));
        max-height: calc(100dvh - 32px - env(safe-area-inset-top, 0px) - env(safe-area-inset-bottom, 0px));
        overflow-y: auto;
        pointer-events: none;
    }
    .toast {
        padding: calc(10px * var(--density)) calc(14px * var(--density));
        border: none;
        border-radius: var(--radius-sm);
        background: var(--bg);
        box-shadow: var(--raised);
        color: var(--text);
        font-family: inherit;
        font-size: 13px;
        text-align: left;
        pointer-events: auto;
        cursor: pointer;
        animation: slide-in 0.15s ease-out;
        word-break: break-word;
        flex-shrink: 0;
    }
    .cleanup-notice { display: flex; align-items: center; gap: 12px; cursor: default; }
    .cleanup-notice > div { flex: 1; min-width: 0; overflow-wrap: anywhere; }
    .cleanup-notice p { margin: 4px 0 0; color: var(--text-sub); }
    .cleanup-notice button { flex-shrink: 0; }
    .toast-error { border-left: 3px solid var(--error); }
    .toast-success { border-left: 3px solid var(--success); }
    .toast-info { border-left: 3px solid var(--accent); }
    @keyframes slide-in {
        from { transform: translateX(16px); opacity: 0; }
        to { transform: translateX(0); opacity: 1; }
    }
</style>
