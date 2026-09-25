<script lang="ts">
    // Pi coding-agent chat tab (TRAE-style). Attached to an active SSH session:
    // spawns a persistent `pi --mode rpc` subprocess on that connection and
    // renders its JSONL event stream as a chat — streaming text, thinking, and
    // tool-call cards. Conversations persist server-side under
    // ~/.pi/agent/sessions/, so reconnecting from another machine to the same
    // host continues the same session (no handoff docs needed).
    import { onMount, onDestroy } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import { listen, type UnlistenFn } from "@tauri-apps/api/event";
    import { t, errMsg } from "../i18n/index.svelte.ts";

    let { tabId, meta }: { tabId: string; meta: Record<string, string> } = $props();

    const sessionId = meta.sessionId ?? "";
    const profileId = meta.profileId ?? "";

    let piId = $state<string | null>(null);
    let status = $state<"idle" | "starting" | "running" | "closed" | "error">("idle");
    let statusMsg = $state("");
    let input = $state("");
    let busy = $state(false);
    let thinkingOpen = $state(true);

    interface SessionInfo {
        path: string;
        name: string;
        cwd: string;
        title: string;
    }
    let sessionList = $state<SessionInfo[]>([]);
    let showSessionList = $state(false);
    let loadingSessions = $state(false);

    interface ToolCallView {
        id: string;
        name: string;
        argsText: string;
        resultText: string;
        isError: boolean;
        streaming: boolean;
    }
    interface MsgView {
        id: string;
        role: "user" | "assistant";
        text: string;
        thinking: string;
        toolCalls: ToolCallView[];
        streaming: boolean;
    }
    let messages = $state<MsgView[]>([]);

    let unlisteners: UnlistenFn[] = [];

    function formatArgs(a: unknown): string {
        if (a == null) return "";
        if (typeof a === "string") return a;
        try { return JSON.stringify(a, null, 2); } catch { return String(a); }
    }
    function resultTextOf(r: unknown): string {
        if (r == null) return "";
        if (typeof r === "string") return r;
        const content = (r as { content?: unknown })?.content;
        if (content != null) {
            const arr = Array.isArray(content) ? content : [content];
            const txt = arr
                .map((c) => (c && typeof c === "object" && (c as { type?: string }).type === "text" ? String((c as { text?: string }).text ?? "") : ""))
                .join("");
            if (txt) return txt;
        }
        try { return JSON.stringify(r); } catch { return String(r); }
    }
    function formatResult(r: unknown): string {
        return resultTextOf(r).slice(0, 4000);
    }

    /** "2026-08-24T09-57-18-091Z_01a0..." → "2026-08-24 09:57" */
    function shortTime(name: string): string {
        const m = name.match(/^(\d{4}-\d{2}-\d{2})T(\d{2})-(\d{2})-\d{2}/);
        if (m) return `${m[1]} ${m[2]}:${m[3]}`;
        return name.slice(0, 16);
    }

    function lastAssistantIdx(): number {
        for (let i = messages.length - 1; i >= 0; i--) if (messages[i].role === "assistant") return i;
        return -1;
    }
    function lastMsg(): MsgView | null {
        const i = lastAssistantIdx();
        return i >= 0 ? messages[i] : null;
    }
    function contentText(content: unknown): string {
        if (typeof content === "string") return content;
        if (Array.isArray(content)) {
            return content
                .map((c) => (c && typeof c === "object" && (c as { type?: string }).type === "text" ? String((c as { text?: string }).text ?? "") : ""))
                .join("");
        }
        return "";
    }
    function contentThinking(content: unknown): string {
        if (Array.isArray(content)) {
            return content
                .map((c) => {
                    if (!c || typeof c !== "object") return "";
                    const t = c as { type?: string };
                    if (t.type === "thinking") return String((c as { thinking?: string; text?: string }).thinking ?? (c as { text?: string }).text ?? "");
                    return "";
                })
                .join("");
        }
        return "";
    }
    function toolCallsOf(content: unknown): ToolCallView[] {
        if (!Array.isArray(content)) return [];
        const out: ToolCallView[] = [];
        for (const c of content) {
            if (c?.type === "toolCall" || c?.type === "tool_call") {
                out.push({
                    id: c.id ?? "",
                    name: c.toolName ?? c.name ?? "",
                    argsText: formatArgs(c.args),
                    resultText: formatResult(c.result),
                    isError: !!c.isError,
                    streaming: false,
                });
            }
        }
        return out;
    }

    function handleLine(line: string) {
        let ev: any;
        try { ev = JSON.parse(line); } catch { return; }
        switch (ev?.type) {
            case "response": onResponse(ev); break; // 命令回执
            case "agent_start": busy = true; break;
            case "agent_end":
            case "agent_settled": busy = false; break;
            case "message_start": onMessageStart(ev.message); break;
            case "message_update": onMessageUpdate(ev.assistantMessageEvent); break;
            case "message_end": onMessageEnd(ev.message); break;
            case "tool_execution_start": onToolStart(ev); break;
            case "tool_execution_update": onToolUpdate(ev); break;
            case "tool_execution_end": onToolEnd(ev); break;
            default: break; // extension_ui_request 等 fire-and-forget UI 事件忽略
        }
    }

    function onResponse(ev: any) {
        if (ev.command === "get_messages" && ev.success && ev.data?.messages) {
            renderHistory(ev.data.messages);
        }
    }

    /** 渲染服务端会话的完整历史（续接/重开时拉取，让聊天区不是空白）。 */
    function renderHistory(list: unknown[]) {
        const next: MsgView[] = [];
        for (const m of list) {
            if (!m || typeof m !== "object") continue;
            const mm = m as { id?: string; role?: string; content?: unknown };
            next.push({
                id: mm.id ?? `h-${Math.random().toString(36).slice(2, 8)}`,
                role: mm.role === "user" ? "user" : "assistant",
                text: contentText(mm.content),
                thinking: contentThinking(mm.content),
                toolCalls: toolCallsOf(mm.content),
                streaming: false,
            });
        }
        messages = next;
    }

    function onMessageStart(m: any) {
        if (!m) return;
        const role = m.role === "user" ? "user" : "assistant";
        // pi 会回显用户消息；send() 已本地 push 一条，同文本则跳过避免重复
        if (role === "user") {
            const last = messages[messages.length - 1];
            if (last?.role === "user" && last.text === contentText(m.content)) return;
        }
        messages.push({
            id: m.id ?? `m-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
            role,
            text: contentText(m.content),
            thinking: contentThinking(m.content),
            toolCalls: [],
            streaming: role === "assistant",
        });
        if (Array.isArray(m.content)) {
            const calls = toolCallsOf(m.content);
            if (calls.length > 0) {
                const msg = messages[messages.length - 1];
                msg.toolCalls = calls;
            }
        }
    }

    function onMessageUpdate(e: any) {
        if (!e) return;
        const msg = lastMsg();
        if (!msg) return;
        switch (e.type) {
            case "text_delta": msg.text += e.delta ?? ""; break;
            case "text_end": msg.streaming = false; break;
            case "thinking_delta": msg.thinking += e.delta ?? ""; break;
            case "toolcall_start":
                msg.toolCalls.push({ id: e.id ?? "", name: e.toolName ?? "", argsText: "", resultText: "", isError: false, streaming: true });
                break;
            case "toolcall_delta":
                if (msg.toolCalls.length > 0) msg.toolCalls[msg.toolCalls.length - 1].argsText += e.delta ?? "";
                break;
            case "toolcall_end":
                if (msg.toolCalls.length > 0) {
                    const tc = msg.toolCalls[msg.toolCalls.length - 1];
                    tc.streaming = false;
                    tc.argsText = formatArgs(e.toolCall?.args ?? e.args);
                }
                break;
        }
    }

    function onMessageEnd(m: any) {
        if (!m) return;
        const i = lastAssistantIdx();
        if (i < 0) return;
        const msg = messages[i];
        msg.text = contentText(m.content);
        msg.thinking = contentThinking(m.content);
        msg.streaming = false;
        const calls = toolCallsOf(m.content);
        if (calls.length > 0) msg.toolCalls = calls;
    }

    function onToolStart(ev: any) {
        const msg = lastMsg();
        if (!msg) return;
        if (ev.toolCallId && msg.toolCalls.some((tc) => tc.id === ev.toolCallId)) return;
        msg.toolCalls.push({
            id: ev.toolCallId ?? "",
            name: ev.toolName ?? "",
            argsText: formatArgs(ev.args),
            resultText: "",
            isError: false,
            streaming: true,
        });
    }
    function onToolUpdate(ev: any) {
        const msg = lastMsg();
        if (!msg) return;
        const tc = msg.toolCalls.find((c) => c.id === ev.toolCallId);
        if (tc) { tc.resultText = formatResult(ev.partialResult); tc.streaming = true; }
    }
    function onToolEnd(ev: any) {
        const msg = lastMsg();
        if (!msg) return;
        const tc = msg.toolCalls.find((c) => c.id === ev.toolCallId);
        if (tc) { tc.resultText = formatResult(ev.result); tc.isError = !!ev.isError; tc.streaming = false; }
    }

    async function start(cont: boolean, sessionPath?: string) {
        status = "starting";
        try {
            const id = await invoke<string>("pi_session_start", {
                sessionId,
                continueSession: cont,
                sessionPath: sessionPath ?? null,
            });
            piId = id;
            status = "running";
            const un1 = await listen<{ pi_id: string; line: string }>(`pi:line:${id}`, (e) => handleLine(e.payload.line));
            const un2 = await listen<{ pi_id: string; status: string; message?: string }>(`pi:status:${id}`, (e) => {
                if (e.payload.status === "closed") { status = "closed"; busy = false; }
                else if (e.payload.status === "error") { status = "error"; statusMsg = e.payload.message ?? ""; busy = false; }
            });
            unlisteners.push(un1, un2);
            // 启动即拉服务端会话历史（续接时显示完整上下文，不是空白页）
            await invoke("pi_session_send", { piId: id, line: JSON.stringify({ type: "get_messages" }) });
        } catch (e) {
            status = "error";
            statusMsg = errMsg(e);
        }
    }

    async function toggleSessionList() {
        if (showSessionList) { showSessionList = false; return; }
        loadingSessions = true;
        try {
            sessionList = await invoke<SessionInfo[]>("pi_session_list", { sessionId });
            showSessionList = true;
        } catch (e) {
            status = "error";
            statusMsg = errMsg(e);
        } finally {
            loadingSessions = false;
        }
    }

    /** 挑选一个历史会话续接：停掉当前进程，用指定会话文件重启。 */
    async function resumeSession(path: string) {
        showSessionList = false;
        if (piId) {
            try { await invoke("pi_session_stop", { piId }); } catch { /* ignore */ }
            piId = null;
        }
        for (const un of unlisteners) { try { un(); } catch { /* ignore */ } }
        unlisteners = [];
        messages = [];
        busy = false;
        await start(false, path);
    }

    /** 删除服务端会话文件（确认后），并刷新列表。 */
    async function deleteSession(path: string, ev: Event) {
        ev.stopPropagation();
        if (!window.confirm(t("pi.sessions_delete_confirm"))) return;
        try {
            await invoke("pi_session_delete", { sessionId, path });
            sessionList = sessionList.filter((s) => s.path !== path);
        } catch (e) {
            status = "error";
            statusMsg = errMsg(e);
        }
    }

    async function send() {
        const text = input.trim();
        if (!text || !piId || status !== "running") return;
        input = "";
        messages.push({ id: `u-${Date.now()}`, role: "user", text, thinking: "", toolCalls: [], streaming: false });
        try {
            await invoke("pi_session_send", { piId, line: JSON.stringify({ type: "prompt", message: text }) });
        } catch (e) {
            status = "error";
            statusMsg = errMsg(e);
        }
    }

    async function stop() {
        if (piId) {
            try { await invoke("pi_session_send", { piId, line: JSON.stringify({ type: "abort" }) }); } catch { /* ignore */ }
        }
    }

    function clearChat() {
        messages = [];
    }

    onMount(() => {
        if (!sessionId) {
            status = "error";
            statusMsg = "no-ssh-session";
            return;
        }
        void start(false);
    });

    onDestroy(() => {
        for (const un of unlisteners) { try { un(); } catch { /* ignore */ } }
        unlisteners = [];
        if (piId) {
            void invoke("pi_session_stop", { piId }).catch(() => {});
        }
    });

    let scroller: HTMLDivElement | undefined = $state();
    $effect(() => {
        messages.length;
        busy;
        scroller?.scrollTo({ top: scroller.scrollHeight });
    });

    function onKeydown(e: KeyboardEvent) {
        if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
            e.preventDefault();
            void send();
        }
    }
</script>

<div class="pi-tab">
    <header class="pi-bar">
        <span class="pi-title">Pi</span>
        <span class="pi-status" class:on={status === "running"} class:err={status === "error"} class:busy={busy}>
            {status === "starting" ? t("pi.starting") : busy ? t("pi.working") : status === "running" ? t("pi.ready") : status === "closed" ? t("pi.closed") : status === "error" ? t("pi.error") : t("pi.idle")}
        </span>
        {#if status === "error" && statusMsg}<span class="pi-status-msg">{statusMsg}</span>{/if}
        <div class="pi-bar-actions">
            <button type="button" class="pi-btn" onclick={() => void start(false)} disabled={status === "starting"}>{t("pi.new_session")}</button>
            <button type="button" class="pi-btn" onclick={() => void start(true)} disabled={status === "starting"}>{t("pi.continue_last")}</button>
            <div class="pi-sessions-wrap">
                <button type="button" class="pi-btn" onclick={toggleSessionList} disabled={status === "starting" || loadingSessions}>
                    {t("pi.sessions")}{loadingSessions ? "…" : ""}
                </button>
                {#if showSessionList}
                    <div class="pi-sessions-drop">
                        {#if sessionList.length === 0}
                            <div class="pi-sessions-empty">{t("pi.sessions_empty")}</div>
                        {:else}
                            {#each sessionList as s, si (s.path)}
                                <div class="pi-session-item">
                                    <button type="button" class="pi-session-open" onclick={() => void resumeSession(s.path)}>
                                        <span class="pi-session-title">{s.title || t("pi.sessions_no_title")}</span>
                                        <span class="pi-session-meta">{s.cwd || t("pi.sessions_unknown_cwd")} · {shortTime(s.name)}</span>
                                    </button>
                                    <button
                                        type="button"
                                        class="pi-session-del"
                                        title={t("pi.sessions_delete")}
                                        aria-label={t("pi.sessions_delete")}
                                        onclick={(e) => void deleteSession(s.path, e)}
                                    >×</button>
                                </div>
                            {/each}
                        {/if}
                    </div>
                {/if}
            </div>
            <button type="button" class="pi-btn" onclick={clearChat} disabled={messages.length === 0}>{t("pi.clear")}</button>
            <button type="button" class="pi-btn" onclick={stop} disabled={!busy}>{t("pi.abort")}</button>
        </div>
    </header>

    <div class="pi-messages" bind:this={scroller}>
        {#if messages.length === 0}
            <div class="pi-empty">
                <div class="pi-empty-title">Pi coding agent</div>
                <div class="pi-empty-sub">{t("pi.empty_hint")}</div>
            </div>
        {/if}
        {#each messages as msg (msg.id)}
            <div class="pi-msg" class:user={msg.role === "user"}>
                <div class="pi-msg-role">{msg.role === "user" ? t("pi.you") : "Pi"}</div>
                {#if msg.thinking}
                    <details class="pi-thinking" open={thinkingOpen}>
                        <summary>{t("pi.thinking")}</summary>
                        <pre>{msg.thinking}</pre>
                    </details>
                {/if}
                {#if msg.text}
                    <div class="pi-text">{msg.text}{msg.streaming ? "▍" : ""}</div>
                {/if}
                {#if msg.toolCalls.length > 0}
                    <div class="pi-tools">
                        {#each msg.toolCalls as tc, ti (tc.id || `${msg.id}-${ti}`)}
                            <details class="pi-tool" class:err={tc.isError}>
                                <summary>
                                    <span class="pi-tool-name">{tc.name || "tool"}</span>
                                    {#if tc.streaming}<span class="pi-tool-stream">…</span>{/if}
                                    {#if tc.isError}<span class="pi-tool-err">✗</span>{/if}
                                </summary>
                                {#if tc.argsText}<pre class="pi-tool-args">{tc.argsText}</pre>{/if}
                                {#if tc.resultText}<pre class="pi-tool-result">{tc.resultText}</pre>{/if}
                            </details>
                        {/each}
                    </div>
                {/if}
            </div>
        {/each}
    </div>

    <footer class="pi-input-row">
        <textarea
            class="pi-input"
            rows="2"
            placeholder={t("pi.placeholder")}
            bind:value={input}
            onkeydown={onKeydown}
        ></textarea>
        <button type="button" class="pi-send" onclick={send} disabled={!input.trim() || status !== "running" || !piId}>{t("pi.send")}</button>
    </footer>
</div>

<style>
    .pi-tab {
        display: flex;
        flex-direction: column;
        height: 100%;
        background: var(--color-panel, #fafafa);
        color: var(--color-text, #222);
        font-size: 13px;
    }
    .pi-bar {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 6px 10px;
        border-bottom: 1px solid var(--color-border, #e4e4e4);
        background: var(--color-bar, #f3f3f3);
        flex: 0 0 auto;
        flex-wrap: wrap;
    }
    .pi-title { font-weight: 600; font-size: 13px; }
    .pi-status {
        font-size: 11px;
        color: var(--color-text-muted, #888);
        border: 1px solid var(--color-border, #e4e4e4);
        border-radius: 9px;
        padding: 1px 8px;
        background: #fff;
    }
    .pi-status.on { color: #2e8b57; border-color: #2e8b57; }
    .pi-status.err { color: #c0392b; border-color: #c0392b; }
    .pi-status.busy { color: #b8860b; border-color: #b8860b; }
    .pi-status-msg { font-size: 11px; color: #c0392b; max-width: 40%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .pi-bar-actions { margin-left: auto; display: flex; gap: 6px; align-items: center; }
    .pi-btn {
        font-size: 11px;
        padding: 3px 10px;
        border: 1px solid var(--color-border, #d0d0d0);
        border-radius: 6px;
        background: #fff;
        cursor: pointer;
    }
    .pi-btn:disabled { opacity: 0.5; cursor: default; }
    .pi-sessions-wrap { position: relative; }
    .pi-sessions-drop {
        position: absolute;
        right: 0;
        top: calc(100% + 4px);
        z-index: 50;
        min-width: 340px;
        max-width: 460px;
        max-height: 320px;
        overflow-y: auto;
        background: #fff;
        border: 1px solid var(--color-border, #d0d0d0);
        border-radius: 8px;
        box-shadow: 0 4px 16px rgba(0, 0, 0, 0.12);
        padding: 4px;
        display: flex;
        flex-direction: column;
        gap: 2px;
    }
    .pi-sessions-empty { padding: 10px; font-size: 12px; color: var(--color-text-muted, #888); }
    .pi-session-item { display: flex; align-items: center; gap: 4px; border-radius: 6px; }
    .pi-session-item:hover { background: #eef2fb; }
    .pi-session-open {
        flex: 1 1 auto;
        min-width: 0;
        display: flex;
        flex-direction: column;
        gap: 2px;
        text-align: left;
        border: none;
        background: transparent;
        padding: 6px 8px;
        cursor: pointer;
    }
    .pi-session-title { font-size: 12px; color: #333; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .pi-session-meta { font-family: ui-monospace, monospace; font-size: 10.5px; color: var(--color-text-muted, #888); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .pi-session-del {
        flex: 0 0 auto;
        border: none;
        background: transparent;
        color: var(--color-text-muted, #888);
        font-size: 13px;
        line-height: 1;
        padding: 4px 8px;
        border-radius: 4px;
        cursor: pointer;
        visibility: hidden;
    }
    .pi-session-item:hover .pi-session-del { visibility: visible; }
    .pi-session-del:hover { color: #c0392b; background: rgba(192, 57, 43, 0.08); }
    .pi-messages {
        flex: 1 1 auto;
        overflow-y: auto;
        padding: 12px;
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .pi-empty { margin: auto; text-align: center; color: var(--color-text-muted, #999); }
    .pi-empty-title { font-size: 15px; font-weight: 600; margin-bottom: 4px; }
    .pi-empty-sub { font-size: 12px; }
    .pi-msg {
        display: flex;
        flex-direction: column;
        gap: 4px;
        max-width: 100%;
    }
    .pi-msg.user { align-items: flex-end; }
    .pi-msg-role { font-size: 11px; color: var(--color-text-muted, #888); }
    .pi-text {
        white-space: pre-wrap;
        word-break: break-word;
        background: #fff;
        border: 1px solid var(--color-border, #ececec);
        border-radius: 8px;
        padding: 8px 10px;
        line-height: 1.55;
        max-width: 100%;
    }
    .pi-msg.user .pi-text { background: #e8f1fb; border-color: #d4e5f7; }
    .pi-thinking {
        background: #fbf7e9;
        border: 1px solid #efe6c8;
        border-radius: 8px;
        padding: 4px 8px;
        font-size: 12px;
    }
    .pi-thinking summary { cursor: pointer; color: #8a6d3b; font-size: 11px; }
    .pi-thinking pre { white-space: pre-wrap; word-break: break-word; margin: 4px 0 0; color: #7a6a44; }
    .pi-tools { display: flex; flex-direction: column; gap: 4px; }
    .pi-tool {
        background: #f6f6f8;
        border: 1px solid #e2e2e8;
        border-radius: 8px;
        padding: 3px 8px;
        font-size: 12px;
    }
    .pi-tool.err { border-color: #e8b4b0; background: #fdf3f2; }
    .pi-tool summary { cursor: pointer; display: flex; align-items: center; gap: 6px; font-family: ui-monospace, monospace; }
    .pi-tool-name { color: #4a5aa8; font-weight: 600; }
    .pi-tool-stream { color: #b8860b; }
    .pi-tool-err { color: #c0392b; font-weight: 700; }
    .pi-tool-args, .pi-tool-result {
        white-space: pre-wrap;
        word-break: break-word;
        margin: 4px 0 2px;
        font-size: 11px;
        color: #444;
        max-height: 200px;
        overflow: auto;
        background: #fff;
        border-radius: 6px;
        padding: 6px 8px;
    }
    .pi-tool-result { color: #2e5d34; }
    .pi-input-row {
        flex: 0 0 auto;
        display: flex;
        gap: 8px;
        padding: 8px 10px;
        border-top: 1px solid var(--color-border, #e4e4e4);
        background: var(--color-bar, #f3f3f3);
    }
    .pi-input {
        flex: 1 1 auto;
        resize: none;
        border: 1px solid var(--color-border, #d0d0d0);
        border-radius: 8px;
        padding: 8px 10px;
        font: inherit;
        font-size: 13px;
        line-height: 1.5;
        background: #fff;
    }
    .pi-send {
        align-self: flex-end;
        padding: 8px 16px;
        border: none;
        border-radius: 8px;
        background: #3b6fd4;
        color: #fff;
        font-size: 13px;
        cursor: pointer;
    }
    .pi-send:disabled { opacity: 0.5; cursor: default; }
</style>
