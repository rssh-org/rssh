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
        border-radius: 6px;
        padding: calc(8px * var(--density)) calc(10px * var(--density));
        margin: calc(4px * var(--density)) 0;
        background: var(--bg);
    }
    .web-card.pending {
        border-left: 3px solid var(--accent);
        background: color-mix(in srgb, var(--accent) 6%, var(--bg));
    }
    .web-card.done { border-left: 3px solid var(--success); }
    .web-card.rejected { opacity: 0.6; border-left: 3px solid var(--text-dim); }

    .head { display: flex; gap: 8px; align-items: center; }
    .tag {
        flex: none;
        font-size: 11px;
        background: var(--accent);
        color: var(--white);
        padding: 1px 6px;
        border-radius: 3px;
        font-weight: 600;
    }
    .target {
        font-family: monospace;
        font-size: 12.5px;
        flex: 1;
        min-width: 0;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }
    .actions { margin-top: 8px; display: flex; gap: 8px; }
    .btn { padding: 4px 12px; border-radius: 4px; cursor: pointer; font-size: 12px; }
    .btn-approve { background: var(--success); color: var(--white); border: none; }
    .btn-approve:disabled { opacity: 0.6; cursor: default; }
    .btn-reject { background: transparent; border: 1px solid var(--text-dim); color: var(--text); }
    .btn-ghost { background: transparent; border: 1px solid var(--divider); color: var(--text); }
    .reject-form { margin-top: 8px; display: flex; gap: 6px; }
    .reject-form input {
        flex: 1; padding: 4px 8px; border: 1px solid var(--divider);
        border-radius: 4px; background: var(--bg); color: var(--text);
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
