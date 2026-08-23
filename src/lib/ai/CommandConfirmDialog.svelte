<script lang="ts">
    import { onDestroy, onMount } from "svelte";
    import * as ai from "./store.svelte.ts";
    import { commandApprovals, isAutoApprovalAllowed } from "./command-approval.ts";
    import type { SessionInstanceRef } from "./session-identity.ts";
    import { t, errMsg } from "../i18n/index.svelte.ts";
    import { toast } from "../stores/toast.svelte.ts";
    import type { AiTargetKind, CommandProposed, CommandResult } from "./types.ts";
    import { isRawDeviceKind } from "./types.ts";

    let { tabId, instanceId, targetKind, targetSessionId, cmd, result, rejected, active } = $props<{
        tabId: string;
        instanceId: string;
        targetKind: AiTargetKind;
        targetSessionId: string | null;
        cmd: CommandProposed;
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
    // Raw devices (serial/telnet) only: the "submit output" button (distinct
    // from terminate) is in flight.
    let submitting = $state(false);
    let eligibilityReady = $state(false);
    let autoApproveEligible = $state(false);

    const sessionRef = (): SessionInstanceRef => ({ tabId, instanceId });

    let isPending = $derived(!result && !rejected);

    function syncExecutionStatus() {
        const status = ai.commandExecutionStatus(sessionRef(), cmd.id);
        transportRunning = status === "running";
        resultDeliveryFailed = status === "delivery_failed";
        executing = status === "running" || status === "reporting" || status === "delivered";
    }

    // 自动批准只由当前可见 tab 发起。ChatPanel 现在会保活隐藏 tab；如果仍在 onMount
    // 无条件批准，后台 tab 的命令会比旧行为更早执行。active 变 true 时 effect 再检查，
    // UI 上"提议→执行"全程可见，审计 trail 与原行为不变。
    //
    // 重入防御：组件可能被销毁重建（chat list 重新 key 等）。
    // 重建实例的 executing=false，单看 executing 拦不住同一命令卡第二次 approve
    // 会被粘到 PTY 两次（rm/reboot 双执行级别的灾难）。用 store 的 per-execution registry
    // （isCommandRunning）守门：命令还在 in-flight 时拒绝再次自动批准。
    //
    // onMount 只负责恢复已在执行的卡片视觉状态。
    onMount(() => {
        const session = sessionRef();
        autoApproveEligible = commandApprovals.eligibleWhileAllowed(
            session,
            cmd.id,
            isAutoApprovalAllowed(ai.settings(), cmd.kind),
        );
        eligibilityReady = true;
        // Command already in flight when this dialog remounts after a keyed list
        // rebuild. Reflect the running state
        // so the card shows Terminate/Submit instead of a stale Approve button
        // (clicking which would be a no-op now that executeCommand guards on the
        // running map, but a dead button is confusing). The original execution
        // still owns the listener/timer and delivers the result.
        if (isPending) {
            syncExecutionStatus();
        }
    });

    onDestroy(() => {
        // Keep guards across ordinary keyed-list remounts, but release them
        // when explicit panel/tab teardown removes the whole conversation.
        // The replacement actor cannot start until teardown finishes and gets
        // a fresh timeline, so no later component can reuse this command card.
        if (!ai.isOpen(tabId)) {
            commandApprovals.clear(sessionRef(), cmd.id);
        }
    });

    // Execution can outlive this component (for example, switch to Audit and
    // back). The registry is reactive, so a remounted card still observes a
    // later running → delivery_failed transition and exposes report-only retry.
    $effect(() => {
        if (isPending) syncExecutionStatus();
    });

    // Eligibility is snapshotted when the command arrives. A later settings
    // enable cannot authorize an old proposal; a later disable can still revoke
    // the captured permission before this hidden tab becomes active.
    $effect(() => {
        if (eligibilityReady && autoApproveEligible) {
            autoApproveEligible = commandApprovals.eligibleWhileAllowed(
                sessionRef(),
                cmd.id,
                isAutoApprovalAllowed(ai.settings(), cmd.kind),
            );
        }
    });

    // 历史卡片没有 kind 字段 → autoApproveAllowed 返回 false → 走人审，符合 fail-safe。
    $effect(() => {
        if (
            active
            && eligibilityReady
            && autoApproveEligible
            && isPending
            && !executing
            && !askingReason
            // No danger mode on raw devices: a bare serial peer (firmware / PLC /
            // bootloader) or a telnet peer (core switch, router) is too sensitive
            // to auto-paste into — and the POSIX-oriented blacklist can't catch
            // network-OS dangers (`reload`, `erase startup-config`). Always ask.
            && !isRawDeviceKind(targetKind)
            && !ai.isCommandRunning(sessionRef(), cmd.id)
            && !commandApprovals.wasAttempted(sessionRef(), cmd.id)
        ) {
            void approve();
        }
    });

    // Result/rejected ends every guard for this exact actor + command card. A later
    // actor in the same tab has another instance id and never shares this entry.
    $effect(() => {
        if (result || rejected) {
            commandApprovals.clear(sessionRef(), cmd.id);
        }
    });

    async function approve() {
        if (executing) return;
        const session = sessionRef();
        const retryingResultDelivery = resultDeliveryFailed;
        // Reserve before the first await. Manual approval counts too: if settings
        // become permissive while it runs, the reactive auto path must not fire.
        if (!retryingResultDelivery) {
            commandApprovals.markAttempted(session, cmd.id);
        }
        resultDeliveryFailed = false;
        executing = true;
        transportRunning = !retryingResultDelivery;
        try {
            const liveTargetSessionId = targetSessionId;
            if (!liveTargetSessionId) throw new Error(t("common.disconnected"));
            await ai.executeCommand(session, cmd.id, cmd.execution, targetKind, liveTargetSessionId);
        } catch (e) {
            console.error("[ai] execute failed:", e);
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
        // executeCommand awaits until the result is delivered, so on success the
        // card is already in its final state — reflect it and clear transient flags.
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
            await ai.rejectCommand(sessionRef(), cmd.id, reason);
            askingReason = false;
            rejectReason = "";
        } catch (e) {
            // Close can win this invoke; the dialog is then gone, but the
            // rejected Promise still needs an owner.
            console.warn("[ai] reject command:", e);
        }
    }

    /** ssh/local 执行中点的"提前终止"：发 Ctrl+C；后续 finish() 上报 early_terminated=true。 */
    async function terminate() {
        if (terminating) return;
        terminating = true;
        try {
            await ai.terminateCommand(sessionRef(), cmd.id);
            syncExecutionStatus();
        } catch (e) {
            console.error("[ai] terminate failed:", e);
            syncExecutionStatus();
            terminating = false;
        }
    }

    /**
     * Raw-device-only "submit output": the user watched the device finish
     * responding. Reports the accumulated buffer as a CLEAN result — no Ctrl+C
     * (nothing to interrupt), not early-terminated. A dedicated button, fully
     * separate from terminate, so neither action is overloaded onto the other.
     */
    async function submit() {
        if (submitting) return;
        submitting = true;
        try {
            await ai.submitCommand(sessionRef(), cmd.id);
            syncExecutionStatus();
        } catch (e) {
            // Match approve()'s feedback — otherwise a failed submit looks like a
            // dead button (user clicked, nothing happened, no clue why).
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

<div class="cmd-card surface-flat" class:pending={isPending} class:done={!!result} class:rejected={!!rejected}>
    <div class="head">
        <span class="tag">
            {t("ai.cmd.proposed.tag")}
        </span>
        <code class="cmd" title={cmd.cmd}>{cmd.cmd}</code>
    </div>
    <div class="meta">
        <div><span class="label">{t("ai.cmd.label.explain")}</span><span class="val" title={cmd.explain}>{cmd.explain}</span></div>
        <div><span class="label">{t("ai.cmd.label.side_effect")}</span><span class="val" title={cmd.side_effect}>{cmd.side_effect}</span></div>
        <div><span class="label">{t("ai.cmd.label.timeout")}</span><span class="val">{cmd.execution.timeout_s}s</span></div>
    </div>

    {#if isPending}
        {#if !askingReason}
            <div class="actions">
                <button class="btn btn-approve" onclick={approve} disabled={executing}>
                    {resultDeliveryFailed ? t("ai.cmd.btn.retry_result") : executing ? t("ai.cmd.btn.executing") : t("ai.cmd.btn.approve")}
                </button>
                {#if transportRunning && isRawDeviceKind(targetKind)}
                    <!-- Raw devices: a dedicated "submit output" button, fully separate
                         from Terminate. The user clicks it when the device has finished
                         responding; it reports the buffer as a clean result. -->
                    <button class="btn btn-submit" onclick={submit} disabled={submitting}>
                        {submitting ? t("ai.cmd.btn.submitting") : t("ai.cmd.btn.submit")}
                    </button>
                {:else if transportRunning}
                    <!-- ack-only 命令（analyze_locally）没 PTY，
                         Terminate 发 Ctrl+C 是 no-op，不该露给用户当 affordance。 -->
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
    .cmd-card {
        border: 1px solid var(--divider);
        border-radius: 10px;
        padding: calc(10px * var(--density)) calc(12px * var(--density));
        margin: calc(2px * var(--density)) 0;
        background: var(--bg);
    }
    /* Status spine + hairline tinted by state (scenes.js tool-card language). */
    .cmd-card.pending {
        border-color: color-mix(in srgb, var(--warning) 30%, var(--divider));
        border-left: 3px solid var(--warning);
        background: color-mix(in srgb, var(--warning) 7%, var(--bg));
    }
    .cmd-card.done {
        border-color: color-mix(in srgb, var(--success) 25%, var(--divider));
        border-left: 3px solid var(--success);
        background: color-mix(in srgb, var(--success) 4%, var(--bg));
    }
    .cmd-card.rejected {
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
        background: color-mix(in srgb, var(--warning) 15%, transparent);
        color: var(--warning);
        border: 1px solid color-mix(in srgb, var(--warning) 38%, transparent);
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
    .label { flex: none; min-width: 50px; color: var(--text-dim); }
    .val {
        flex: 1;
        min-width: 0;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }
    .actions { margin-top: 10px; display: flex; gap: 8px; }
    .btn { padding: 4px 12px; border-radius: 6px; cursor: pointer; }
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
    /* Serial "submit output" — a positive completion action, so green like approve
       (the two never co-occur: approve shows pre-exec, submit shows during exec). */
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
