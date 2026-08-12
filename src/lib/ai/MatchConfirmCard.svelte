<script lang="ts">
    import { onDestroy, onMount } from "svelte";
    import * as ai from "./store.svelte.ts";
    import { commandApprovals, isAutoApprovalAllowed } from "./command-approval.ts";
    import type { SessionInstanceRef } from "./session-identity.ts";
    import { t, errMsg } from "../i18n/index.svelte.ts";
    import { toast } from "../stores/toast.svelte.ts";
    import type { AiTargetKind, CommandResult, MatchProposal } from "./types.ts";
    import { isRawDeviceKind } from "./types.ts";

    // Dedicated PTY execution card for match_file (read-only search). Like
    // PatchConfirmCard it reuses the shared runner: approve hands
    // proposal.execution to executeCommand. Read-only and low-risk, so it has
    // its own auto-approval toggle (auto_match_file) and is the only tool that
    // reasonably defaults to auto-run.
    let { tabId, instanceId, targetKind, targetSessionId, proposal, result, rejected, active } = $props<{
        tabId: string;
        instanceId: string;
        targetKind: AiTargetKind;
        targetSessionId: string | null;
        proposal: MatchProposal;
        result?: CommandResult;
        rejected?: { reason: string };
        active: boolean;
    }>();

    let askingReason = $state(false);
    let rejectReason = $state("");
    let executing = $state(false);
    let transportRunning = $state(false);
    let resultDeliveryFailed = $state(false);
    let terminating = $state(false);
    let submitting = $state(false);
    let eligibilityReady = $state(false);
    let autoApproveEligible = $state(false);

    const sessionRef = (): SessionInstanceRef => ({ tabId, instanceId });
    let isPending = $derived(!result && !rejected);

    function syncExecutionStatus() {
        const status = ai.commandExecutionStatus(sessionRef(), proposal.id);
        transportRunning = status === "running";
        resultDeliveryFailed = status === "delivery_failed";
        executing = status === "running" || status === "reporting" || status === "delivered";
    }

    onMount(() => {
        const session = sessionRef();
        autoApproveEligible = commandApprovals.eligibleWhileAllowed(
            session,
            proposal.id,
            isAutoApprovalAllowed(ai.settings(), "match_file"),
        );
        eligibilityReady = true;
        if (isPending) syncExecutionStatus();
    });

    onDestroy(() => {
        if (!ai.isOpen(tabId)) {
            commandApprovals.clear(sessionRef(), proposal.id);
        }
    });

    $effect(() => {
        if (isPending) syncExecutionStatus();
    });

    $effect(() => {
        if (eligibilityReady && autoApproveEligible) {
            autoApproveEligible = commandApprovals.eligibleWhileAllowed(
                sessionRef(),
                proposal.id,
                isAutoApprovalAllowed(ai.settings(), "match_file"),
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
            && !isRawDeviceKind(targetKind)
            && !ai.isCommandRunning(sessionRef(), proposal.id)
            && !commandApprovals.wasAttempted(sessionRef(), proposal.id)
        ) {
            void approve();
        }
    });

    $effect(() => {
        if (result || rejected) {
            commandApprovals.clear(sessionRef(), proposal.id);
        }
    });

    async function approve() {
        if (executing) return;
        const session = sessionRef();
        const retryingResultDelivery = resultDeliveryFailed;
        if (!retryingResultDelivery) {
            commandApprovals.markAttempted(session, proposal.id);
        }
        resultDeliveryFailed = false;
        executing = true;
        transportRunning = !retryingResultDelivery;
        try {
            const liveTargetSessionId = targetSessionId;
            if (!liveTargetSessionId) throw new Error(t("common.disconnected"));
            await ai.executeCommand(session, proposal.id, proposal.execution, targetKind, liveTargetSessionId);
        } catch (e) {
            console.error("[ai] match execute failed:", e);
            syncExecutionStatus();
            toast.error(t(
                resultDeliveryFailed
                    ? "ai.cmd.alert.result_delivery_failed"
                    : "ai.cmd.alert.exec_failed",
                { error: errMsg(e) },
            ));
            terminating = false;
            submitting = false;
            return;
        }
        syncExecutionStatus();
        terminating = false;
        submitting = false;
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
            console.warn("[ai] reject match:", e);
        }
    }

    async function terminate() {
        if (terminating) return;
        terminating = true;
        try {
            await ai.terminateCommand(sessionRef(), proposal.id);
            syncExecutionStatus();
        } catch (e) {
            console.error("[ai] terminate failed:", e);
            syncExecutionStatus();
            terminating = false;
        }
    }

    async function submit() {
        if (submitting) return;
        submitting = true;
        try {
            await ai.submitCommand(sessionRef(), proposal.id);
            syncExecutionStatus();
        } catch (e) {
            console.error("[ai] submit failed:", e);
            syncExecutionStatus();
            toast.error(t(
                resultDeliveryFailed
                    ? "ai.cmd.alert.result_delivery_failed"
                    : "ai.cmd.alert.submit_failed",
                { error: errMsg(e) },
            ));
            submitting = false;
        }
    }
</script>

<div class="match-card surface-flat" class:pending={isPending} class:done={!!result} class:rejected={!!rejected}>
    <div class="head">
        <span class="tag">{t("ai.match.tag")}</span>
        <code class="cmd" title={proposal.cmd}>{proposal.cmd}</code>
    </div>
    <div class="meta">
        <div><span class="label">{t("ai.match.file")}</span><code class="val" title={proposal.path}>{proposal.path}</code></div>
        <div><span class="label">{t("ai.match.find")}</span><code class="val mono" title={proposal.find}>{proposal.find}</code></div>
        <div><span class="label">{t("ai.match.context")}</span><span class="val">±{proposal.before}/{proposal.after}</span></div>
        <div><span class="label">{t("ai.cmd.label.timeout")}</span><span class="val">{proposal.execution.timeout_s}s</span></div>
    </div>

    {#if isPending}
        {#if !askingReason}
            <div class="actions">
                <button class="btn btn-approve" onclick={approve} disabled={executing}>
                    {resultDeliveryFailed ? t("ai.cmd.btn.retry_result") : executing ? t("ai.cmd.btn.executing") : t("ai.cmd.btn.approve")}
                </button>
                {#if transportRunning && isRawDeviceKind(targetKind)}
                    <button class="btn btn-submit" onclick={submit} disabled={submitting}>
                        {submitting ? t("ai.cmd.btn.submitting") : t("ai.cmd.btn.submit")}
                    </button>
                {:else if transportRunning}
                    <button class="btn btn-terminate" onclick={terminate} disabled={terminating}>
                        {terminating ? t("ai.cmd.btn.terminating") : t("ai.cmd.btn.terminate")}
                    </button>
                {:else if !executing && !resultDeliveryFailed}
                    <button class="btn btn-reject" onclick={reject}>{t("ai.cmd.btn.reject")}</button>
                {/if}
            </div>
            {#if transportRunning}
                <div class="hint">{targetKind === "serial" ? t("ai.cmd.hint.executing_serial") : targetKind === "telnet" ? t("ai.cmd.hint.executing_telnet") : t("ai.cmd.hint.executing")}</div>
            {/if}
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
            <div class="result-meta">
                <span>exit={result.exit_code}</span>
                <span>{result.duration_ms}ms</span>
                {#if result.timed_out}<span class="warn">{t("ai.cmd.warn.timed_out")}</span>{/if}
                {#if result.early_terminated}<span class="warn">{t("ai.cmd.warn.early_terminated")}</span>{/if}
                {#if result.truncated_bytes > 0}<span class="warn">{t("ai.cmd.warn.truncated", { bytes: result.truncated_bytes })}</span>{/if}
            </div>
            <pre class="output">{result.output || t("ai.cmd.empty_output")}</pre>
        </div>
    {/if}
</div>

<style>
    .match-card {
        border: 1px solid var(--divider);
        border-radius: 6px;
        padding: calc(8px * var(--density)) calc(10px * var(--density));
        margin: calc(4px * var(--density)) 0;
        background: var(--bg);
    }
    .match-card.pending {
        border-left: 3px solid var(--accent);
        background: color-mix(in srgb, var(--accent) 4%, var(--bg));
    }
    .match-card.done { border-left: 3px solid var(--success); }
    .match-card.rejected { opacity: 0.6; border-left: 3px solid var(--text-dim); }

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
    .cmd {
        font-family: monospace;
        font-size: 12.5px;
        flex: 1;
        min-width: 0;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }
    .meta { font-size: 12px; margin-top: 6px; color: var(--text-dim); }
    .meta > div { display: flex; gap: 8px; }
    .label { flex: none; min-width: 60px; color: var(--text-dim); }
    .val {
        flex: 1;
        min-width: 0;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }
    .val.mono { font-family: monospace; font-size: 11.5px; }

    .actions { margin-top: 8px; display: flex; gap: 8px; }
    .btn { padding: 4px 12px; border-radius: 4px; cursor: pointer; font-size: 12px; }
    .btn-approve { background: var(--success); color: var(--white); border: none; }
    .btn-approve:disabled { opacity: 0.6; cursor: default; }
    .btn-reject { background: transparent; border: 1px solid var(--text-dim); color: var(--text); }
    .btn-terminate { background: var(--warning); color: var(--black); border: none; }
    .btn-terminate:disabled { opacity: 0.6; cursor: default; }
    .btn-submit { background: var(--success); color: var(--white); border: none; }
    .btn-submit:disabled { opacity: 0.6; cursor: default; }
    .btn-ghost { background: transparent; border: 1px solid var(--divider); color: var(--text); }
    .reject-form { margin-top: 8px; display: flex; gap: 6px; }
    .reject-form input {
        flex: 1; padding: 4px 8px; border: 1px solid var(--divider);
        border-radius: 4px; background: var(--bg); color: var(--text);
    }
    .rejected-note { font-size: 12px; margin-top: 6px; color: var(--text-dim); }
    .hint { font-size: 11px; color: var(--text-dim); margin-top: 4px; font-style: italic; }
    .result { margin-top: 8px; }
    .result-meta { display: flex; gap: 8px; font-size: 11px; color: var(--text-dim); }
    .result-meta .warn { color: var(--warning); }
    .output {
        margin-top: 4px;
        padding: 6px 8px;
        background: color-mix(in srgb, var(--text) 5%, var(--bg));
        border-radius: 4px;
        font-family: monospace;
        font-size: 12px;
        max-height: 240px;
        overflow: auto;
        white-space: pre-wrap;
        word-break: break-all;
    }
</style>
