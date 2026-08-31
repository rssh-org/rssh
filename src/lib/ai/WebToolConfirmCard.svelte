<script lang="ts">
    import { onDestroy, onMount } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import * as ai from "./store.svelte.ts";
    import { commandApprovals, isAutoApprovalAllowed } from "./command-approval.ts";
    import type { SessionInstanceRef } from "./session-identity.ts";
    import { t, errMsg } from "../i18n/index.svelte.ts";
    import { toast } from "../stores/toast.svelte.ts";
    import type { WebToolProposal, WebToolResult } from "./types.ts";

    // Dedicated approval card for web_search / web_fetch. Independent data
    // model (WebToolProposal / WebToolResult) — does NOT reuse
    // CommandProposed / CommandResult, so it carries no command-only dead
    // fields. Approve acks via ai_command_result (the shared approve channel,
    // by id); reject via rejectCommand.
    let { tabId, instanceId, proposal, result, rejected, active } = $props<{
        tabId: string;
        instanceId: string;
        proposal: WebToolProposal;
        result?: WebToolResult;
        rejected?: { reason: string };
        active: boolean;
    }>();

    let askingReason = $state(false);
    let rejectReason = $state("");
    let executing = $state(false);
    let eligibilityReady = $state(false);
    let autoApproveEligible = $state(false);

    const sessionRef = (): SessionInstanceRef => ({ tabId, instanceId });
    let isPending = $derived(!result && !rejected);
    let isSearch = $derived(proposal.kind === "web_search");

    onMount(() => {
        autoApproveEligible = commandApprovals.eligibleWhileAllowed(
            sessionRef(),
            proposal.id,
            isAutoApprovalAllowed(ai.settings(), proposal.kind),
        );
        eligibilityReady = true;
        if (isPending && commandApprovals.isAcknowledged(sessionRef(), proposal.id)) {
            executing = true;
        }
    });

    onDestroy(() => {
        if (!ai.isOpen(tabId)) commandApprovals.clear(sessionRef(), proposal.id);
    });

    // A later settings disable revokes the arrival snapshot before any auto run.
    $effect(() => {
        if (eligibilityReady && autoApproveEligible) {
            autoApproveEligible = commandApprovals.eligibleWhileAllowed(
                sessionRef(),
                proposal.id,
                isAutoApprovalAllowed(ai.settings(), proposal.kind),
            );
        }
    });

    // Auto-approve only from the visible tab; re-entry guarded by the registry
    // so a remount can never double-ack the same proposal.
    $effect(() => {
        if (
            active
            && eligibilityReady
            && autoApproveEligible
            && isPending
            && !executing
            && !askingReason
            && !commandApprovals.isAcknowledged(sessionRef(), proposal.id)
            && !commandApprovals.wasAttempted(sessionRef(), proposal.id)
        ) {
            void approve();
        }
    });

    $effect(() => {
        if (result || rejected) commandApprovals.clear(sessionRef(), proposal.id);
    });

    async function approve() {
        if (executing) return;
        const session = sessionRef();
        if (commandApprovals.isAcknowledged(session, proposal.id)) return;
        commandApprovals.markAttempted(session, proposal.id);
        executing = true;
        commandApprovals.markAcknowledged(session, proposal.id);
        try {
            await invoke("ai_command_result", {
                tabId,
                instanceId,
                toolCallId: proposal.id,
                exitCode: 0,
                output: "",
                timedOut: false,
                earlyTerminated: false,
            });
        } catch (e) {
            commandApprovals.clearAcknowledged(session, proposal.id);
            executing = false;
            toast.error(t("ai.cmd.alert.exec_failed", { error: errMsg(e) }));
        }
    }

    async function reject() {
        if (!askingReason) {
            askingReason = true;
            return;
        }
        const reason = rejectReason.trim();
        if (!reason) return;
        try {
            await ai.rejectCommand(sessionRef(), proposal.id, reason);
            askingReason = false;
            rejectReason = "";
        } catch (e) {
            // Keep the rejection form open so the user can retry; just surface
            // the failure instead of swallowing it (mirrors approve()'s catch).
            toast.error(t("ai.cmd.alert.exec_failed", { error: errMsg(e) }));
        }
    }
</script>

<div class="web-card surface-flat" class:pending={isPending} class:done={!!result && result.ok} class:rejected={!!rejected}>
    <div class="head">
        <span class="tag">{t(isSearch ? "ai.webcmd.search" : "ai.webcmd.fetch")}</span>
        <code class="target" title={proposal.target}>{proposal.target}</code>
    </div>

    {#if isPending}
        {#if !askingReason}
            <div class="actions">
                <button class="btn btn-approve" onclick={approve} disabled={executing}>
                    {executing ? t("ai.webcmd.running") : t("ai.webcmd.allow")}
                </button>
                <button class="btn btn-reject" onclick={reject} disabled={executing}>{t("ai.webcmd.reject")}</button>
            </div>
        {:else}
            <div class="reject-form">
                <input
                    bind:value={rejectReason}
                    placeholder={t("ai.cmd.reject.placeholder")}
                    onkeydown={(e) => { if (e.key === "Enter") reject(); }}
                />
                <button class="btn" onclick={reject} disabled={!rejectReason.trim()}>{t("ai.cmd.reject.submit")}</button>
                <button class="btn btn-ghost" onclick={() => (askingReason = false)}>{t("ai.cmd.reject.cancel")}</button>
            </div>
        {/if}
    {:else if rejected}
        <div class="rejected-note">{t("ai.cmd.rejected_note", { reason: rejected.reason })}</div>
    {:else if result}
        <div class="result">
            <span class="output" title={result.summary}>{result.summary || t("ai.webcmd.done")}</span>
            {#if !result.ok}
                <span class="warn">{t("ai.webcmd.failed")}</span>
            {/if}
            <span class="dur">{result.duration_ms}ms</span>
        </div>
    {/if}
</div>

<style>
    .web-card {
        border: 1px solid var(--divider);
        border-radius: 10px;
        padding: calc(10px * var(--density)) calc(12px * var(--density));
        margin: calc(2px * var(--density)) 0;
        background: var(--bg);
    }
    /* Status spine + hairline tinted by state (scenes.js tool-card language);
       --purple = the AI panel's identity accent (global.css). */
    .web-card.pending {
        border-color: color-mix(in srgb, var(--purple) 30%, var(--divider));
        border-left: 3px solid var(--purple);
        background: color-mix(in srgb, var(--purple) 7%, var(--bg));
    }
    .web-card.done {
        border-color: color-mix(in srgb, var(--success) 25%, var(--divider));
        border-left: 3px solid var(--success);
        background: color-mix(in srgb, var(--success) 4%, var(--bg));
    }
    .web-card.rejected {
        border-color: color-mix(in srgb, var(--text-dim) 30%, var(--divider));
        border-left: 3px solid var(--text-dim);
        opacity: 0.6;
    }

    .head { display: flex; gap: 8px; align-items: center; }
    .tag {
        flex: none;
        font-size: 10px;
        font-weight: 700;
        text-transform: uppercase;
        letter-spacing: 0.06em;
        background: color-mix(in srgb, var(--purple) 15%, transparent);
        color: var(--purple);
        border: 1px solid color-mix(in srgb, var(--purple) 38%, transparent);
        padding: 2px 7px;
        border-radius: 5px;
    }
    .target {
        font-family: var(--term-font);
        font-size: 12px;
        flex: 1;
        min-width: 0;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        padding: 3px 8px;
        border-radius: 6px;
        background: color-mix(in srgb, var(--black) 22%, var(--bg));
    }
    .actions { margin-top: 10px; display: flex; gap: 8px; }
    .btn { padding: 4px 12px; border-radius: 6px; cursor: pointer; font-size: 12px; }
    /* Tinted outline buttons (scenes.js approve-btn language) — translucent, so
       the global .btn neumorphic hover shadow must stay off (it would bleed
       through the tint). */
    .btn-approve {
        background: color-mix(in srgb, var(--success) 15%, transparent);
        color: var(--success);
        border: 1px solid color-mix(in srgb, var(--success) 45%, transparent);
        font-weight: 600;
        box-shadow: none;
    }
    .btn-approve:hover:not(:disabled) {
        background: color-mix(in srgb, var(--success) 24%, transparent);
        box-shadow: none;
    }
    .btn-approve:disabled { opacity: 0.6; cursor: default; }
    .btn-reject { background: transparent; border: 1px solid var(--text-dim); color: var(--text); box-shadow: none; }
    .btn-ghost { background: transparent; border: 1px solid var(--divider); color: var(--text); box-shadow: none; }
    .reject-form { margin-top: 10px; display: flex; gap: 6px; }
    .reject-form input {
        flex: 1; padding: 5px 9px; border: 1px solid var(--divider);
        border-radius: 6px;
        background: color-mix(in srgb, var(--black) 18%, var(--bg));
        color: var(--text);
        transition: border-color 150ms ease, box-shadow 150ms ease;
    }
    .reject-form input:focus {
        outline: none;
        border-color: color-mix(in srgb, var(--purple) 55%, transparent);
        box-shadow: 0 0 0 3px color-mix(in srgb, var(--purple) 18%, transparent);
    }
    .rejected-note { font-size: 12px; margin-top: 6px; color: var(--text-dim); }
    .result {
        margin-top: 8px;
        display: flex;
        gap: 8px;
        align-items: baseline;
        font-size: 11.5px;
        color: var(--text-dim);
        font-variant-numeric: tabular-nums;
    }
    .result .output {
        color: var(--text);
        flex: 1;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .result .warn { color: var(--error); }
</style>
