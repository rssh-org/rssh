<script lang="ts">
    import { onDestroy, onMount } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import * as ai from "./store.svelte.ts";
    import { commandApprovals, isAutoApprovalAllowed } from "./command-approval.ts";
    import type { SessionInstanceRef } from "./session-identity.ts";
    import { t, errMsg } from "../i18n/index.svelte.ts";
    import { toast } from "../stores/toast.svelte.ts";
    import type { AnalyzeProposal, AnalyzeResult } from "./types.ts";

    // Dedicated approval card for analyze_locally (spawn a new analysis window).
    // Independent data model (AnalyzeProposal / AnalyzeResult). Ack-only: approve
    // just acks via ai_command_result; the backend opens the window itself.
    let { tabId, instanceId, proposal, result, rejected, active } = $props<{
        tabId: string;
        instanceId: string;
        proposal: AnalyzeProposal;
        result?: AnalyzeResult;
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

    onMount(() => {
        autoApproveEligible = commandApprovals.eligibleWhileAllowed(
            sessionRef(),
            proposal.id,
            isAutoApprovalAllowed(ai.settings(), "analyze_locally"),
        );
        eligibilityReady = true;
        if (isPending && commandApprovals.isAcknowledged(sessionRef(), proposal.id)) {
            executing = true;
        }
    });

    onDestroy(() => {
        if (!ai.isOpen(tabId)) commandApprovals.clear(sessionRef(), proposal.id);
    });

    $effect(() => {
        if (eligibilityReady && autoApproveEligible) {
            autoApproveEligible = commandApprovals.eligibleWhileAllowed(
                sessionRef(),
                proposal.id,
                isAutoApprovalAllowed(ai.settings(), "analyze_locally"),
            );
        }
    });

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
            console.warn("[ai] reject analyze:", e);
        }
    }
</script>

<div class="an-card surface-flat" class:pending={isPending} class:done={!!result} class:rejected={!!rejected}>
    <div class="head">
        <span class="tag">{t("ai.an.tag")}</span>
        <code class="target" title={proposal.local_path}>{proposal.local_path}</code>
    </div>
    <div class="meta">
        <span class="label">{t("ai.an.task")}</span>
        <span class="task" title={proposal.task}>{proposal.task}</span>
    </div>

    {#if isPending}
        {#if !askingReason}
            <div class="actions">
                <button class="btn btn-approve" onclick={approve} disabled={executing}>
                    {executing ? t("ai.an.running") : t("ai.an.allow")}
                </button>
                <button class="btn btn-reject" onclick={reject}>{t("ai.an.reject")}</button>
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
            <span class="output" title={result.summary}>{result.summary || t("ai.an.done")}</span>
            {#if !result.ok}
                <span class="warn">{t("ai.an.failed")}</span>
            {/if}
            <span class="dur">{result.duration_ms}ms</span>
        </div>
    {/if}
</div>

<style>
    .an-card {
        border: 1px solid var(--divider);
        border-radius: 6px;
        padding: calc(8px * var(--density)) calc(10px * var(--density));
        margin: calc(4px * var(--density)) 0;
        background: var(--bg);
    }
    .an-card.pending {
        border-left: 3px solid var(--accent);
        background: color-mix(in srgb, var(--accent) 6%, var(--bg));
    }
    .an-card.done { border-left: 3px solid var(--success); }
    .an-card.rejected { opacity: 0.6; border-left: 3px solid var(--text-dim); }

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
    .meta {
        margin-top: 4px;
        display: flex;
        gap: 6px;
        align-items: baseline;
        font-size: 11px;
        color: var(--text-dim);
    }
    .meta .label { flex: none; }
    .meta .task {
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
