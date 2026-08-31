<script lang="ts">
    import * as ai from "./store.svelte.ts";
    import type { AiTargetKind, ChatItem, ConversationMeta } from "./types.ts";
    import CommandConfirmDialog from "./CommandConfirmDialog.svelte";
    import WebToolConfirmCard from "./WebToolConfirmCard.svelte";
    import DownloadConfirmCard from "./DownloadConfirmCard.svelte";
    import AnalyzeConfirmCard from "./AnalyzeConfirmCard.svelte";
    import PatchConfirmCard from "./PatchConfirmCard.svelte";
    import MatchConfirmCard from "./MatchConfirmCard.svelte";
    import AuditPanel from "./AuditPanel.svelte";
    import Modal from "../components/Modal.svelte";
    import DangerModeToggle from "./DangerModeToggle.svelte";
    import { renderMarkdown } from "./markdown.ts";
    import { formatTokenCount } from "./tokens.ts";
    import { t, errMsg } from "../i18n/index.svelte.ts";
    import { toast } from "../stores/toast.svelte.ts";
    import { writeText as writeClipboard } from "../clipboard.ts";
    import { onMount } from "svelte";

    // tabId 是 AI 会话身份（切 tab / 重连不丢；显式关闭面板时结束）。
    // targetId 是当前 SSH/PTY session_id —— 给 executeCommand 路由 ssh_write/pty_write 用。
    // 重连后 targetId 会换（前端 prop 自动跟随），tabId 不变。
    let { tabId, targetKind, targetId, active } = $props<{
        tabId: string;
        targetKind: AiTargetKind;
        targetId: string | null;
        active: boolean;
    }>();

    type PanelOwner = Readonly<{
        tabId: string;
        targetKind: AiTargetKind;
        lease: ai.SessionLease;
    }>;
    const snapshotOwner = (): PanelOwner => ({
        tabId,
        targetKind,
        lease: ai.captureSessionLease(tabId),
    });

    let inputText = $state("");
    let auditOpen = $state(false);
    let busy = $state(false);
    let banner = $state<string | null>(null);
    let inputEl = $state<HTMLTextAreaElement | null>(null);
    let chatBoxEl = $state<HTMLDivElement | null>(null);
    let rollingBack = $state(false);
    let rollbackDialog = $state<{
        owner: PanelOwner;
        instanceId: string;
        userMessageIndex: number;
        text: string;
    } | null>(null);

    let session = $derived(ai.sessionForTab(tabId));
    let items: ChatItem[] = $derived(ai.chatItems(tabId));
    // 流式响应进行中 —— send 按钮换成"停止"按钮。依赖 items 变化重算（last item 的 streaming flag）。
    let streaming = $derived(ai.isStreaming(tabId));
    // 危险模式标记 —— 用户在 AI Settings 里切换后，标题旁的红色后缀立刻同步。
    // 走 ai.settings() 读 store 的 $state，自动响应式（不需要手动 loadSettings 触发）。
    let dangerMode = $derived(ai.settings()?.danger_mode === true);
    // 本会话累计 token 用量（结束会话/新开会话即归零重计）。
    let tokens = $derived(ai.tokenUsage(tabId));
    // Currently running model: prefer the model the active session actually started
    // with (authoritative — a later settings change doesn't affect a live session);
    // fall back to the configured model (what will run) when there's no session yet.
    // Empty string when neither is known — the .model span still works as the spring.
    let currentModel = $derived(session?.model ?? ai.settings()?.model ?? "");

    // 该 profile 下持久化的历史对话 —— 仅会话未启动时展示（picker）。
    // null = 还没加载完，与空数组（确无历史）区分，避免列表闪现。
    let conversations = $state<ConversationMeta[] | null>(null);

    onMount(async () => {
        // 只拉 settings（提示词标题的 danger 旗等要它）。不在这里预启 session ——
        // shell 探测已移到 SSH 连接成功时跑（TerminalPane），开 panel 不再为探测拉
        // 起 actor。会话改为首次发消息时（send → ensureSession）惰性启动。
        if (!ai.settings()) {
            try { await ai.loadSettings(); } catch { /* 静默 */ }
        }
    });

    // 色条"发送到 AI"塞进来的输入：消费一次就清掉，避免切 tab 回来又灌一遍。
    // 无条件先把 $state 读进局部变量，确保依赖被跟踪（即便为 null）——否则首跑
    // 为空时 Svelte 5 不会登记对 _prefill 的依赖，后续 prefill 永远触发不了。
    $effect(() => {
        const p = ai.pendingPrefill(tabId);
        if (!p) return;
        inputText = p.text;
        auditOpen = false;
        ai.clearPrefill(tabId);
        if (active) inputEl?.focus();
    });

    // 固定挂载的隐藏面板不能保留全局 modal：否则 A 隐藏后仍会拦截 B 的 Esc。
    // 草稿等面板内状态继续保留，只关闭越出 panel 边界的确认层。
    $effect(() => {
        if (active) return;
        rollbackDialog = null;
    });

    // 历史对话随当前 target（同一 tab 重连时 session id 会变）重新加载。
    // seq 守卫丢弃旧连接的迟到响应，避免它覆盖新连接的列表。
    let convSeq = 0;
    $effect(() => {
        const kind = targetKind;
        const id = targetId;
        const seq = ++convSeq;
        if (session) return; // 会话已存在：picker 不展示，无需拉取
        conversations = null;
        if (!id) return; // 断线期间保活面板，但不拿 null target 查历史
        // 两个回调都同时 gate seq + session：用户开面板后立刻发消息，会话先
        // 起来、列表请求后返回 —— 此时 picker 已无意义，迟到的失败不该在活跃
        // 对话里弹错误 banner（seq 不增长，单靠它挡不住这条路径）。
        ai.listConversations(kind, id)
            .then((list) => { if (seq === convSeq && !session) conversations = list; })
            .catch((e) => {
                // 加载失败不挡新对话，但必须上 banner —— 静默置空会让"有历史但
                // 后端抽风"看起来跟"确无历史"一模一样，用户以为记录丢了。
                console.error("[ai] list conversations:", e);
                if (seq === convSeq && !session) {
                    conversations = [];
                    banner = errMsg(e);
                }
            });
    });

    $effect(() => {
        items.length;
        if (chatBoxEl) {
            queueMicrotask(() => { chatBoxEl!.scrollTop = chatBoxEl!.scrollHeight; });
        }
    });

    /** 单飞 guard：onMount 预热 + send() 都会调 ensureSession，并发时两次都看不到
     *  session 存在（store 写入是 startSession 完成后才落），双 startSession 后端会
     *  报 session_already_exists。promise 复用：第二个调用方等同一个 promise 完成。 */
    let ensureInFlight: Promise<void> | null = null;

    function liveTargetId(): string {
        if (!targetId) throw new Error(t("common.disconnected"));
        return targetId;
    }

    /** start/resume 期间若连接重建，TerminalPane 可能在 actor 尚未落 store 时跳过
     *  rebind；启动完成后在这里补一次，保证 actor 绑定当前连接而不是死句柄。 */
    async function rebindIfNeeded(owner: PanelOwner, startedTargetId: string) {
        const latestTargetId = liveTargetId();
        if (latestTargetId !== startedTargetId) {
            await ai.rebindTarget(owner.tabId, owner.targetKind, latestTargetId, owner.lease);
        }
    }

    /** 没 session 就先启动；启动失败抛错。
     *  skill 固定 general —— 用户自定义 skill 已自动拼进 master prompt，让 LLM 自己路由。
     *  远端 shell 探测不在这里 —— 它在 SSH 连接时已跑过并写进 profile 缓存，
     *  startSession 从缓存读初始 shell（缓存 miss 则 POSIX 兜底）。 */
    async function ensureSession(owner: PanelOwner): Promise<void> {
        if (ai.sessionForTab(owner.tabId)) return;
        if (ensureInFlight) return ensureInFlight;
        ensureInFlight = (async () => {
            const settings = ai.settings() ?? await ai.loadSettings();
            if (!settings.provider) {
                throw new Error(t("ai.error.no_provider"));
            }
            if (!settings.has_api_key) {
                throw new Error(t("ai.error.no_api_key"));
            }
            // targetId 是连接句柄：同一 tab 重连后应使用此刻的最新值，不能在点击时
            // 冻结旧句柄；tabId / targetKind 才是这次动作不可变的 owner。
            const startedTargetId = liveTargetId();
            await ai.startSession({
                tabId: owner.tabId,
                targetKind: owner.targetKind,
                targetId: startedTargetId,
                skill: "general",
                provider: settings.provider, model: settings.model,
                lease: owner.lease,
            });
            await rebindIfNeeded(owner, startedTargetId);
        })();
        try {
            await ensureInFlight;
        } finally {
            ensureInFlight = null;
        }
    }

    // 同一行的 resume / delete 互斥：删除进行中点恢复同一行会产生可避免的
    // not_found 报错。按行互斥（不全局禁）—— 删 A 的几十毫秒里恢复 B 是合法操作。
    let deletingId = $state<string | null>(null);

    /** 点历史对话：actor 带旧 history 出生，UI 灌回存储的 timeline，直接可续聊。 */
    async function resumeConversation(id: string) {
        const owner = snapshotOwner();
        if (busy || session || deletingId === id) return;
        banner = null;
        busy = true;
        try {
            const settings = ai.settings() ?? await ai.loadSettings();
            if (!settings.provider) {
                throw new Error(t("ai.error.no_provider"));
            }
            if (!settings.has_api_key) {
                throw new Error(t("ai.error.no_api_key"));
            }
            const startedTargetId = liveTargetId();
            await ai.resumeSession({
                tabId: owner.tabId,
                targetKind: owner.targetKind,
                targetId: startedTargetId,
                skill: "general",
                provider: settings.provider, model: settings.model,
                lease: owner.lease,
            }, id);
            await rebindIfNeeded(owner, startedTargetId);
        } catch (e: any) {
            console.error("[ai] resume failed:", e);
            banner = errMsg(e);
        } finally {
            busy = false;
        }
    }

    async function deleteConversation(id: string) {
        if (busy || deletingId) return;
        deletingId = id;
        try {
            await ai.deleteConversation(id);
            conversations = (conversations ?? []).filter((c) => c.id !== id);
        } catch (e) {
            console.error("[ai] delete conversation:", e);
            banner = errMsg(e);
        } finally {
            deletingId = null;
        }
    }

    function fmtDate(ms: number) {
        return new Date(ms).toLocaleString();
    }

    async function send() {
        const owner = snapshotOwner();
        const text = inputText.trim();
        if (!text || busy) return;
        banner = null;
        busy = true;
        try {
            await ensureSession(owner);
            inputText = "";
            await ai.sendMessage(owner.tabId, text, owner.lease);
        } catch (e: any) {
            console.error("[ai] send failed:", e);
            banner = errMsg(e);
        } finally {
            busy = false;
        }
    }

    /** 显式关面板 = 结束并归档当前会话；重开回到首次打开状态。 */
    function closePanel() {
        void ai.closePanel(tabId).catch((e) => {
            console.warn("[ai] close panel session:", e);
            toast.error(errMsg(e));
        });
    }

    /** New session: end + archive the current conversation but keep the panel
     *  open — the view falls back to the initial picker state. */
    function newSession() {
        auditOpen = false;
        banner = null;
        void ai.endConversation(tabId).catch((e) => {
            console.warn("[ai] end conversation:", e);
            toast.error(errMsg(e));
        });
    }

    /** 打断当前流式响应；会话上下文保留，用户可立刻发下一条纠正。 */
    async function stopStreaming() {
        const owner = snapshotOwner();
        if (!ai.sessionForTab(owner.tabId)) return;
        try {
            await ai.cancelStream(owner.tabId, owner.lease);
        } catch (e) {
            // 不能只 console.error 就完事——失败的话用户还卡在 streaming/disabled 状态，
            // 看不到任何错误反馈。复用 banner 让用户知道"停止没生效，再点一次或刷新"。
            console.error("[ai] cancel stream:", e);
            banner = errMsg(e);
        }
    }

    function onKeyDown(e: KeyboardEvent) {
        if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            send();
        }
    }

    function fmt(ts: number) {
        return new Date(ts).toLocaleTimeString();
    }

    async function copyUserMessage(text: string) {
        try {
            await writeClipboard(text);
        } catch (error) {
            toast.error(errMsg(error));
        }
    }

    function userMessageIndexAt(itemIndex: number): number {
        return items.slice(0, itemIndex).filter((item) => item.kind === "user").length;
    }

    function openRollbackDialog(itemIndex: number, text: string) {
        if (rollingBack || rollbackDialog || !session) return;
        rollbackDialog = {
            owner: snapshotOwner(),
            instanceId: session.instance_id,
            userMessageIndex: userMessageIndexAt(itemIndex),
            text,
        };
    }

    function closeRollbackDialog() {
        rollbackDialog = null;
    }

    async function confirmRollback() {
        const target = rollbackDialog;
        closeRollbackDialog();
        if (
            !target
            || rollingBack
            || ai.sessionForTab(target.owner.tabId)?.instance_id !== target.instanceId
        ) return;
        rollingBack = true;
        try {
            if (ai.isStreaming(target.owner.tabId)) {
                await ai.cancelStream(target.owner.tabId, target.owner.lease);
            }
            await ai.rollbackContext(
                target.owner.tabId,
                target.userMessageIndex,
                target.text,
                target.owner.lease,
            );
        } catch (error) {
            console.error("[ai] rollback context:", error);
            toast.error(errMsg(error));
        } finally {
            rollingBack = false;
        }
    }
</script>

<div class="ai-panel">
    <div class="toolbar">
        <!-- Current model: left-aligned, single line, ellipsis on overflow (full
             text on hover). Also the flex spring (flex:1) that pushes the controls
             to the right — replaces the old empty .grow spacer. -->
        <span class="model" title={currentModel}>{currentModel}</span>
        <span class="tokens" title={t("ai.toolbar.tokens_tip", { tin: tokens.tokens_in, tout: tokens.tokens_out })}>
            ↑{formatTokenCount(tokens.tokens_in)} ↓{formatTokenCount(tokens.tokens_out)}
        </span>
        <!-- Audit log toggle: file-text icon in chat view, chat bubble in audit view (= go back).
             Toolbar controls render unconditionally (stable layout); they disable until the
             session lazy-starts on first send — no actor, nothing to audit. -->
        <button class="btn-icon" onclick={() => (auditOpen = !auditOpen)} disabled={!session}
                title={auditOpen ? t("ai.toolbar.back_to_chat") : t("ai.toolbar.audit")}
                aria-label={auditOpen ? t("ai.toolbar.back_to_chat") : t("ai.toolbar.audit")}>
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                {#if auditOpen}
                    <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>
                {:else}
                    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
                    <polyline points="14 2 14 8 20 8"/>
                    <line x1="16" y1="13" x2="8" y2="13"/>
                    <line x1="16" y1="17" x2="8" y2="17"/>
                    <polyline points="10 9 8 9"/>
                {/if}
            </svg>
        </button>
        <!-- Danger-mode toggle: always visible, selected (red) when ON. The toggle
             logic + confirm modal live in DangerModeToggle (shared with AiSettings —
             one safety contract); here we only render the icon. No disabled={!session}
             — danger_mode is a global setting, settable before the session starts. -->
        <DangerModeToggle {active} onError={(m) => (banner = m)}>
            {#snippet trigger(requestToggle, saving)}
                <button class="btn-icon danger-toggle" class:on={dangerMode}
                        onclick={requestToggle} disabled={saving}
                        title={dangerMode ? t("ai.title.danger_tip") : t("ai.toolbar.danger_enable")}
                        aria-label={t("ai.toolbar.danger_aria")} aria-pressed={dangerMode}>
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/>
                        <line x1="12" y1="9" x2="12" y2="13"/>
                        <line x1="12" y1="17" x2="12.01" y2="17"/>
                    </svg>
                </button>
            {/snippet}
        </DangerModeToggle>
        <!-- New session: archive the current conversation and return to the
             picker state; the panel stays open. Disabled when there is no
             live conversation to end. -->
        <button class="btn-icon" onclick={newSession} disabled={!session}
                title={t("ai.toolbar.new_session")} aria-label={t("ai.toolbar.new_session")}>
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect x="3" y="3" width="18" height="18" rx="2"/>
                <line x1="12" y1="8" x2="12" y2="16"/>
                <line x1="8" y1="12" x2="16" y2="12"/>
            </svg>
        </button>
        <button class="btn-icon" onclick={closePanel} title={t("ai.toolbar.close_session")} aria-label={t("ai.toolbar.close_session")}>×</button>
    </div>

    {#if banner}
        <div class="banner">
            <span>{banner}</span>
            <button class="btn-icon" onclick={() => (banner = null)}>×</button>
        </div>
    {/if}

    {#if auditOpen && session}
        <AuditPanel {tabId} />
    {:else}
        <div class="chat" bind:this={chatBoxEl}>
            {#each items as item, i (i)}
                <div class="item item-{item.kind}">
                    {#if item.kind === "user"}
                        <div class="ts">{fmt(item.at)}</div>
                        <div class="user-message">
                            <div class="message-actions">
                                <button class="message-action" onclick={() => copyUserMessage(item.text)}
                                        title={t("ai.message.copy")} aria-label={t("ai.message.copy")}>
                                    <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <rect x="9" y="9" width="13" height="13" rx="2"/>
                                        <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                                    </svg>
                                </button>
                                <button class="message-action rollback" onclick={() => openRollbackDialog(i, item.text)}
                                        disabled={rollingBack} title={t("ai.message.rollback")}
                                        aria-label={t("ai.message.rollback")}>
                                    <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <path d="M9 14 4 9l5-5"/>
                                        <path d="M4 9h10a6 6 0 0 1 6 6v5"/>
                                    </svg>
                                </button>
                            </div>
                            <div class="bubble user">{item.text}</div>
                        </div>
                    {:else if item.kind === "assistant"}
                        <div class="ts">{fmt(item.at)}</div>
                        <!-- eslint-disable-next-line svelte/no-at-html-tags -->
                        <div class="bubble assistant md" class:streaming={item.streaming} class:cancelled={item.cancelled}>
                            {#if item.text}
                                {@html renderMarkdown(item.text)}
                            {:else if !item.cancelled}
                                …
                            {/if}
                            {#if item.cancelled}
                                <span class="cancelled-tag">{t("ai.bubble.cancelled")}</span>
                            {/if}
                        </div>
                    {:else if item.kind === "web_tool" && session}
                        {#key item.proposal.id}
                            <WebToolConfirmCard
                                {tabId}
                                instanceId={session.instance_id}
                                proposal={item.proposal}
                                result={item.result}
                                rejected={item.rejected}
                                {active}
                            />
                        {/key}
                    {:else if item.kind === "download" && session}
                        {#key item.proposal.id}
                            <DownloadConfirmCard
                                {tabId}
                                instanceId={session.instance_id}
                                proposal={item.proposal}
                                result={item.result}
                                rejected={item.rejected}
                                {active}
                            />
                        {/key}
                    {:else if item.kind === "analyze" && session}
                        {#key item.proposal.id}
                            <AnalyzeConfirmCard
                                {tabId}
                                instanceId={session.instance_id}
                                proposal={item.proposal}
                                result={item.result}
                                rejected={item.rejected}
                                {active}
                            />
                        {/key}
                    {:else if item.kind === "match" && session}
                        {#key item.proposal.id}
                            <MatchConfirmCard
                                {tabId}
                                instanceId={session.instance_id}
                                targetKind={targetKind}
                                targetSessionId={targetId}
                                proposal={item.proposal}
                                result={item.result}
                                rejected={item.rejected}
                                {active}
                            />
                        {/key}
                    {:else if item.kind === "patch" && session}
                        {#key item.proposal.id}
                            <PatchConfirmCard
                                {tabId}
                                instanceId={session.instance_id}
                                targetKind={targetKind}
                                targetSessionId={targetId}
                                proposal={item.proposal}
                                result={item.result}
                                rejected={item.rejected}
                                {active}
                            />
                        {/key}
                    {:else if item.kind === "command" && session}
                        {#key item.cmd.id}
                            <CommandConfirmDialog
                                {tabId}
                                instanceId={session.instance_id}
                                targetKind={targetKind}
                                targetSessionId={targetId}
                                cmd={item.cmd}
                                result={item.result}
                                rejected={item.rejected}
                                {active}
                            />
                        {/key}
                    {:else if item.kind === "error"}
                        <div class="bubble error">{item.text}</div>
                    {:else if item.kind === "note"}
                        <div class="bubble note">{item.text}</div>
                    {/if}
                </div>
            {/each}
            {#if items.length === 0 && !session}
                <div class="placeholder dim">
                    <p>{t("ai.placeholder.welcome")}</p>
                    <p class="hint">{t("ai.placeholder.example_hint")}</p>
                    <p class="hint">{t("ai.placeholder.confirm_hint")}</p>
                </div>
                {#if conversations && conversations.length > 0}
                    <div class="history">
                        <div class="history-title">{t("ai.history.title")}</div>
                        {#each conversations as c (c.id)}
                            <div class="history-row">
                                <button class="history-item" onclick={() => resumeConversation(c.id)}
                                        disabled={busy || deletingId === c.id} title={t("ai.history.resume_tip")}>
                                    <span class="history-name">{c.title || t("ai.history.untitled")}</span>
                                    <span class="history-time">{fmtDate(c.updated_at)}</span>
                                </button>
                                <!-- 删除全局互斥（deletingId 只能追踪一个 in-flight），禁用范围
                                     必须跟守卫一致：删除进行中所有删除按钮都禁，恢复按钮仍按行。 -->
                                <button class="btn-icon history-del" onclick={() => deleteConversation(c.id)}
                                        disabled={busy || deletingId !== null}
                                        title={t("ai.history.delete")} aria-label={t("ai.history.delete")}>×</button>
                            </div>
                        {/each}
                    </div>
                {/if}
            {/if}
        </div>

        <div class="input-area">
            <textarea
                bind:this={inputEl}
                bind:value={inputText}
                placeholder={busy ? (session ? t("ai.input.replying") : t("ai.input.starting")) : (streaming ? t("ai.input.replying") : t("ai.input.placeholder"))}
                onkeydown={onKeyDown}
                disabled={busy}
                readonly={streaming}
            ></textarea>
            {#if streaming}
                <button class="btn btn-sm btn-stop" onclick={stopStreaming} title={t("ai.input.stop")}>
                    {t("ai.input.stop")}
                </button>
            {:else}
                <button class="btn btn-sm btn-primary" onclick={send} disabled={!inputText.trim() || busy}>
                    {busy && !session ? t("ai.input.starting_short") : t("ai.input.send")}
                </button>
            {/if}
        </div>
    {/if}
</div>

{#if rollbackDialog}
    <Modal onClose={closeRollbackDialog} class="stack"
           aria-labelledby="rollback-dialog-title" aria-describedby="rollback-dialog-body">
        <h3 id="rollback-dialog-title" class="dialog-title">{t("ai.message.rollback_confirm_title")}</h3>
        <div id="rollback-dialog-body" class="dialog-body">{t("ai.message.rollback_confirm")}</div>
        <div class="modal-actions">
            <button class="btn btn-sm" onclick={closeRollbackDialog}>{t("common.cancel")}</button>
            <button class="btn btn-sm btn-danger" onclick={confirmRollback}>
                {t("ai.message.rollback_confirm_action")}
            </button>
        </div>
    </Modal>
{/if}

<style>
    .ai-panel {
        display: flex;
        flex-direction: column;
        height: 100%;
        background: var(--bg);
        border-left: 1px solid var(--divider);
        border-right: 1px solid var(--divider);
    }
    .toolbar {
        display: flex; align-items: center; gap: 8px;
        padding: 8px; border-bottom: 1px solid var(--divider);
        flex-shrink: 0;
    }
    .model {
        flex: 1;
        min-width: 0;
        font-size: 11px;
        font-family: var(--term-font);
        color: var(--text-dim);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }
    /* AI identity marker (scenes.js ai-dot language): a small purple dot
       with a soft glow in front of the model name. Purely decorative —
       renders even before the model name is known. */
    .model::before {
        content: "";
        display: inline-block;
        width: 6px; height: 6px; border-radius: 50%;
        margin-right: 6px;
        background: var(--purple);
        box-shadow: 0 0 6px color-mix(in srgb, var(--purple) 80%, transparent);
    }
    .tokens {
        font-size: 10.5px;
        font-family: var(--term-font);
        color: var(--text-dim);
        white-space: nowrap;
        flex-shrink: 0;
    }
    .btn-primary { background: var(--accent); color: var(--white); border-color: var(--accent); }
    .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }
    /* Never let the composer's growing textarea stretch or squeeze the button. */
    .btn-primary, .btn-stop { flex-shrink: 0; }
    .btn-stop {
        background: var(--error);
        color: var(--white);
        border-color: var(--error);
        cursor: pointer;
    }
    .btn-stop:hover { opacity: 0.85; }
    .btn-ghost { background: transparent; }
    .btn-icon {
        background: transparent; border: none;
        font-size: 18px; cursor: pointer;
        color: var(--text); padding: 4px 6px;
        display: inline-flex; align-items: center; justify-content: center;
        line-height: 1;
        border-radius: 4px;
    }
    .btn-icon:hover {
        background: color-mix(in srgb, var(--text) 8%, transparent);
        color: var(--text);
    }
    .btn-icon:disabled {
        opacity: 0.35;
        cursor: default;
    }
    .btn-icon:disabled:hover { background: transparent; }
    /* Danger-mode toggle, selected state: red icon + red-tinted fill so it reads
       as "on" among the otherwise-neutral toolbar icons. The :hover rule keeps it
       red (overriding .btn-icon:hover's neutral color via higher specificity). */
    .danger-toggle.on {
        color: var(--error);
        background: color-mix(in srgb, var(--error) 14%, transparent);
    }
    .danger-toggle.on:hover {
        color: var(--error);
        background: color-mix(in srgb, var(--error) 22%, transparent);
    }
    .banner {
        display: flex; align-items: center; gap: 8px;
        padding: 8px 12px;
        background: color-mix(in srgb, var(--error) 18%, var(--bg));
        color: var(--error);
        border-bottom: 1px solid var(--divider);
        font-size: 12px;
        flex-shrink: 0;
    }
    .banner span { flex: 1; word-break: break-word; }

    .placeholder {
        padding: 24px; text-align: center;
        color: var(--text-dim);
        line-height: 1.7;
    }
    .placeholder.dim { font-size: 13px; padding: 40px 28px 12px; }
    .hint { font-size: 12px; }

    /* 历史对话 picker —— 仅空状态（无会话）时出现在欢迎语下方。 */
    .history { padding: 0 16px; display: flex; flex-direction: column; gap: 2px; }
    .history-title {
        font-size: 11px; font-weight: 600; color: var(--text-dim);
        text-transform: uppercase; letter-spacing: 0.05em;
        margin: 8px 0 4px;
    }
    .history-row { display: flex; align-items: center; gap: 2px; }
    .history-item {
        flex: 1; min-width: 0;
        display: flex; align-items: baseline; gap: 8px;
        padding: 5px 8px;
        background: transparent; border: none; cursor: pointer;
        border-radius: 4px; color: var(--text);
        text-align: left; font-size: 12.5px;
        border-radius: 6px;
    }
    .history-item:hover { background: color-mix(in srgb, var(--text) 8%, transparent); }
    .history-item:disabled { opacity: 0.5; cursor: default; }
    .history-del:hover { color: var(--error); }
    .history-name {
        flex: 1; min-width: 0;
        overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    }
    .history-time {
        font-size: 10.5px; color: var(--text-dim);
        font-family: var(--term-font); flex-shrink: 0;
    }
    .history-del { font-size: 14px; padding: 2px 5px; color: var(--text-dim); }

    .chat {
        flex: 1; overflow-y: auto; min-height: 0; padding: 10px 12px;
        display: flex; flex-direction: column; gap: 10px;
    }
    /* Bubble entrance (scenes.js language) — transform/opacity only, no
       layout impact, so the scroll-to-bottom on new items is unaffected. */
    .item {
        display: flex; flex-direction: column; gap: 2px;
        animation: bubble-in 240ms cubic-bezier(0.22, 1, 0.36, 1) both;
    }
    @keyframes bubble-in {
        from { opacity: 0; transform: translateY(6px); }
        to { opacity: 1; transform: translateY(0); }
    }
    .user-message {
        display: flex; align-items: center; justify-content: flex-end; gap: 4px;
    }
    .message-actions {
        display: flex; gap: 1px;
        opacity: 0; pointer-events: none;
        transition: opacity 120ms ease;
    }
    .item-user:hover .message-actions,
    .item-user:focus-within .message-actions {
        opacity: 1; pointer-events: auto;
    }
    .message-action {
        width: 24px; height: 24px; padding: 0;
        display: inline-flex; align-items: center; justify-content: center;
        flex-shrink: 0;
        border: 0; border-radius: 4px; background: transparent;
        color: var(--text-dim); cursor: pointer;
    }
    .message-action:hover {
        color: var(--text);
        background: color-mix(in srgb, var(--text) 8%, transparent);
    }
    .message-action.rollback:hover { color: var(--error); }
    .message-action:disabled { opacity: 0.4; cursor: default; }
    @media (hover: none), (any-pointer: coarse) {
        .user-message {
            flex-direction: column;
            align-items: flex-end;
        }
        .message-actions { opacity: 1; pointer-events: auto; }
        .message-action { width: 44px; height: 44px; }
        .message-actions { order: 2; }
        .bubble.user { order: 1; }
    }
    .ts {
        font-size: 10px; color: var(--text-dim);
        font-family: var(--term-font); letter-spacing: 0.03em;
    }
    /* User timestamps sit over their right-aligned bubble; assistant ones
       over the left-aligned bubble — the column reads as two rails. */
    .item-user .ts { align-self: flex-end; margin-right: 2px; }
    .bubble {
        padding: 7px 11px; border-radius: 10px;
        max-width: 92%; word-break: break-word; white-space: pre-wrap;
        font-size: 13px; line-height: 1.5;
    }
    /* Translucent tint + hairline border instead of a solid accent block;
       the sharp corner on the tail side anchors each speaker. */
    .bubble.user {
        background: color-mix(in srgb, var(--accent) 24%, transparent);
        border: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
        color: var(--text);
        border-radius: 10px 10px 4px 10px;
    }
    .bubble.assistant {
        background: color-mix(in srgb, var(--text) 7%, var(--bg));
        border: 1px solid color-mix(in srgb, var(--text) 11%, transparent);
        align-self: flex-start;
        border-radius: 10px 10px 10px 4px;
    }
    .bubble.assistant.streaming {
        position: relative;
    }
    /* 用户打断的响应：气泡尾部跟一个本地化小徽章，区别于"AI 自己结束的对话"。
       徽章本身在 ChatPanel 模板里用 i18n 渲染，避免把英文 marker 硬塞进 LLM 输出文本。 */
    .cancelled-tag {
        display: inline-block;
        margin-left: 6px;
        padding: 1px 6px;
        border-radius: 4px;
        background: color-mix(in srgb, var(--text-dim) 18%, transparent);
        color: var(--text-dim);
        font-size: 10.5px;
        font-weight: 500;
        vertical-align: middle;
    }
    .bubble.assistant.streaming::after {
        content: "▋";
        display: inline-block;
        margin-left: 2px;
        animation: blink 1s steps(2, start) infinite;
        color: var(--purple);
    }
    @keyframes blink {
        to { visibility: hidden; }
    }
    /* Placed AFTER the animated rules: same specificity means the later
       declaration wins, so an earlier block would be overridden by the
       plain animation rules above. */
    @media (prefers-reduced-motion: reduce) {
        .item { animation: none; }
        .bubble.assistant.streaming::after { animation: none; }
    }
    /* Markdown 内容样式 — 极致紧凑 */
    /* 关键：覆盖 .bubble 默认的 pre-wrap。marked 输出的 HTML 标签间有 source-only `\n`，
       pre-wrap 会把那些 `\n` 渲染成可见空行——经典 bug，markdown 气泡必须用 normal。 */
    .bubble.md { line-height: 1.5; font-size: 12.5px; white-space: normal; }
    .bubble.md :global(> *:first-child) { margin-top: 0; }
    .bubble.md :global(> *:last-child) { margin-bottom: 0; }
    .bubble.md :global(p) { margin: 0; }
    .bubble.md :global(p + p) { margin-top: 2px; }
    .bubble.md :global(br) { line-height: 1; }
    .bubble.md :global(code) {
        background: color-mix(in srgb, var(--text) 12%, transparent);
        padding: 1px 4px; border-radius: 3px;
        font-family: var(--term-font); font-size: 11.5px;
    }
    /* Code blocks read as dark insets (scenes.js tool-args language). */
    .bubble.md :global(pre) {
        background: color-mix(in srgb, var(--black) 25%, var(--bg));
        padding: 6px 8px; border-radius: 6px;
        overflow-x: auto; font-size: 11.5px;
        margin: 3px 0; line-height: 1.35;
    }
    .bubble.md :global(pre code) { background: transparent; padding: 0; font-size: inherit; }
    .bubble.md :global(ul), .bubble.md :global(ol) { margin: 2px 0; padding-left: 18px; }
    .bubble.md :global(li) { margin: 0; }
    .bubble.md :global(li > p) { margin: 0; }
    .bubble.md :global(li > ul), .bubble.md :global(li > ol) { margin: 0; }
    .bubble.md :global(strong) { font-weight: 600; }
    .bubble.md :global(em) { font-style: italic; }
    .bubble.md :global(a) { color: var(--accent); }
    .bubble.md :global(h1),
    .bubble.md :global(h2),
    .bubble.md :global(h3),
    .bubble.md :global(h4) {
        margin: 6px 0 2px; font-weight: 600; line-height: 1.25;
    }
    .bubble.md :global(:first-child:is(h1, h2, h3, h4)) { margin-top: 0; }
    .bubble.md :global(h1) { font-size: 14px; }
    .bubble.md :global(h2) { font-size: 13px; }
    .bubble.md :global(h3), .bubble.md :global(h4) { font-size: 12.5px; }
    .bubble.md :global(blockquote) {
        border-left: 3px solid color-mix(in srgb, var(--purple) 45%, transparent);
        padding-left: 8px; margin: 3px 0;
        color: var(--text-dim);
    }
    .bubble.md :global(hr) {
        border: 0; border-top: 1px solid var(--divider);
        margin: 6px 0;
    }
    .bubble.md :global(table) {
        border-collapse: collapse; margin: 3px 0; font-size: 11.5px;
    }
    .bubble.md :global(th) { background: color-mix(in srgb, var(--text) 6%, transparent); }
    .bubble.md :global(th), .bubble.md :global(td) {
        border: 1px solid var(--divider); padding: 2px 6px;
    }
    .bubble.error {
        background: color-mix(in srgb, var(--error) 15%, var(--bg));
        border: 1px solid color-mix(in srgb, var(--error) 35%, transparent);
        color: var(--error);
        font-size: 12px;
    }
    .bubble.note {
        background: transparent;
        color: var(--text-dim);
        font-size: 12px;
        font-style: italic;
        align-self: center;
    }

    .input-area {
        display: flex; align-items: flex-end; gap: 8px; padding: 10px;
        border-top: 1px solid var(--divider);
        flex-shrink: 0;
    }
    /* Dark inset composer (scenes.js ai-input language); focus ring carries
       the panel's purple AI identity. */
    textarea {
        flex: 1; min-height: 36px; max-height: 120px; resize: none;
        padding: 8px 10px; border: 1px solid var(--divider);
        border-radius: 8px;
        background: color-mix(in srgb, var(--black) 18%, var(--bg));
        color: var(--text);
        font-family: inherit; font-size: 13px; line-height: 1.45;
        transition: border-color 150ms ease, box-shadow 150ms ease;
    }
    textarea:focus {
        outline: none;
        border-color: color-mix(in srgb, var(--purple) 55%, transparent);
        box-shadow: 0 0 0 3px color-mix(in srgb, var(--purple) 18%, transparent);
    }

    /* Rollback confirmation modal — shell lives in Modal.svelte, typography
       in global .dialog-title/.dialog-body; only the multi-line body is local. */
    .dialog-body {
        white-space: pre-line;
    }
</style>
