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
        border-radius: 10px;
        padding: calc(10px * var(--density)) calc(12px * var(--density));
        margin: calc(2px * var(--density)) 0;
        background: var(--bg);
    }
    /* Status spine + hairline tinted by state (scenes.js tool-card language);
       --purple = the AI panel's identity accent (global.css). */
    .match-card.pending {
        border-color: color-mix(in srgb, var(--purple) 30%, var(--divider));
        border-left: 3px solid var(--purple);
        background: color-mix(in srgb, var(--purple) 7%, var(--bg));
    }
    .match-card.done {
        border-color: color-mix(in srgb, var(--success) 25%, var(--divider));
        border-left: 3px solid var(--success);
        background: color-mix(in srgb, var(--success) 4%, var(--bg));
    }
    .match-card.rejected {
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
    .cmd {
        font-family: monospace;
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
    .btn-terminate {
        background: color-mix(in srgb, var(--warning) 15%, transparent);
        color: var(--warning);
        border: 1px solid color-mix(in srgb, var(--warning) 45%, transparent);
        font-weight: 600;
        box-shadow: none;
    }
    .btn-terminate:hover:not(:disabled) {
        background: color-mix(in srgb, var(--warning) 24%, transparent);
        box-shadow: none;
    }
    .btn-terminate:disabled { opacity: 0.6; cursor: default; }
    .btn-submit {
        background: color-mix(in srgb, var(--success) 15%, transparent);
        color: var(--success);
        border: 1px solid color-mix(in srgb, var(--success) 45%, transparent);
        font-weight: 600;
        box-shadow: none;
    }
    .btn-submit:hover:not(:disabled) {
        background: color-mix(in srgb, var(--success) 24%, transparent);
        box-shadow: none;
    }
    .btn-submit:disabled { opacity: 0.6; cursor: default; }
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
    .hint { font-size: 11px; color: var(--text-dim); margin-top: 4px; font-style: italic; }
    .result { margin-top: 8px; }
    .result-meta {
        display: flex; gap: 8px; font-size: 11px; color: var(--text-dim);
        font-family: monospace; font-variant-numeric: tabular-nums;
    }
    .result-meta .warn { color: var(--warning); }
    .output {
        margin-top: 6px;
        padding: 8px 10px;
        background: color-mix(in srgb, var(--black) 25%, var(--bg));
        border-radius: 6px;
        font-family: monospace;
        font-size: 12px;
        line-height: 1.4;
        max-height: 240px;
        overflow: auto;
        white-space: pre-wrap;
        word-break: break-all;
    }
</style>
