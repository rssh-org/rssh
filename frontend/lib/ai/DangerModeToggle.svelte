<!-- Shared automatic-mode toggle. Enabling confirms automatic approval of the
     selected tools; disabling takes effect immediately. Settings and toolbar
     controls share this logic through the trigger snippet. -->
<script lang="ts" module>
    // danger_mode 是全局设置；所有 ChatPanel/AiSettings 实例共享一次写入闸门。
    // 切 tab 不能让第二个 toggle 绕过第一个仍在进行的 save。
    let toggleInFlight: Promise<void> | null = null;
</script>

<script lang="ts">
    import type { Snippet } from "svelte";
    import * as ai from "./store.svelte.ts";
    import { t, errMsg } from "../i18n/index.svelte.ts";
    import Modal from "../components/Modal.svelte";
    import AppIcon from "../components/AppIcon.svelte";

    let { trigger, onError, active = true }: {
        // trigger(requestToggle, saving): the caller wires these onto its control.
        trigger: Snippet<[() => void, boolean]>;
        // Raw error message; each surface routes it to its own error UI.
        onError?: (msg: string) => void;
        active?: boolean;
    } = $props();

    let showDialog = $state(false);
    let saving = $state(false);

    function requestToggle() {
        if (saving || toggleInFlight) return;
        if (ai.settings()?.danger_mode === true) {
            void apply(false); // disabling → immediate
        } else {
            showDialog = true; // enabling → confirm first
        }
    }

    async function apply(wantOn: boolean) {
        if (toggleInFlight) return;
        saving = true;
        showDialog = false;
        const operation = ai.saveSettings({ dangerMode: wantOn });
        toggleInFlight = operation;
        try {
            await operation;
        } catch (e) {
            console.error("[ai] toggle automatic mode:", e);
            onError?.(errMsg(e));
        } finally {
            if (toggleInFlight === operation) toggleInFlight = null;
            saving = false;
        }
    }

    $effect(() => {
        if (!active) showDialog = false;
    });
</script>

{@render trigger(requestToggle, saving)}

{#if showDialog}
    <Modal onClose={() => (showDialog = false)} class="stack"
           aria-labelledby="auto-mode-confirm-title" aria-describedby="auto-mode-confirm-body">
        <h3 id="auto-mode-confirm-title" class="title">
            <AppIcon name="auto" size={18} />
            {t("ai.settings.danger.confirm_title")}
        </h3>
        <div id="auto-mode-confirm-body" class="body">{t("ai.settings.danger.confirm_body")}</div>
        <div class="modal-actions">
            <button class="btn btn-sm" onclick={() => (showDialog = false)}>{t("common.cancel")}</button>
            <button class="btn btn-sm btn-accent" onclick={() => apply(true)} disabled={saving}>
                {t("ai.settings.danger.confirm_enable")}
            </button>
        </div>
    </Modal>
{/if}

<style>
    .title {
        display: flex;
        align-items: center;
        gap: 7px;
        font-size: 16px;
        color: var(--accent);
        font-weight: 700;
    }
    .body {
        font-size: 13px;
        color: var(--text);
        line-height: 1.55;
        white-space: pre-line;
    }
</style>
