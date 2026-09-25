<script lang="ts">
    // OpenCode coding-agent chat tab (TRAE / VS Code openchamber style).
    // Attached to an active SSH session: the backend starts (or reuses) a
    // headless `opencode serve` on the remote host, forwards its port locally,
    // and streams the global `/event` bus. This tab renders the conversation
    // for one opencode session — text / thinking / tool cards, session picker,
    // create & delete. Conversations persist in opencode.db server-side, so
    // reconnecting from another machine to the same host continues the same
    // session (no handoff docs needed).
    import { onMount, onDestroy } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import { listen, type UnlistenFn } from "@tauri-apps/api/event";
    import { t, errMsg } from "../i18n/index.svelte.ts";

    let { tabId, meta }: { tabId: string; meta: Record<string, string> } = $props();

    const sessionId = meta.sessionId ?? "";

    let ocId = $state<string | null>(null);
    let status = $state<"idle" | "starting" | "running" | "error">("idle");
    let statusMsg = $state("");
    let input = $state("");
    let busy = $state(false);
    let thinkingOpen = $state(true);
    /** 当前正在查看/对话的 opencode 会话 id */
    let currentSession = $state<string | null>(null);

    interface OcSessionMeta {
        id: string;
        title: string;
        directory: string;
        time: string;
    }
    let sessionList = $state<OcSessionMeta[]>([]);
    let showSessionList = $state(false);
    let loadingSessions = $state(false);

    interface PartView {
        id: string;
        type: string;
        text: string;
        thinking: string;
        tool: string;
        toolState: string;
        input: string;
        output: string;
        isError: boolean;
    }
    interface MsgView {
        id: string;
        role: "user" | "assistant";
        parts: PartView[];
        streaming: boolean;
    }
    let messages = $state<MsgView[]>([]);

    let unlisteners: UnlistenFn[] = [];

    function shortTime(iso: string): string {
        // 毫秒时间戳（opencode time.created）
        if (/^\d{13}$/.test(iso)) {
            const d = new Date(Number(iso));
            if (!isNaN(d.getTime())) {
                const p = (n: number) => String(n).padStart(2, "0");
                return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
            }
        }
        const m = iso.match(/^(\d{4}-\d{2}-\d{2})T(\d{2}):(\d{2})/);
        if (m) return `${m[1]} ${m[2]}:${m[3]}`;
        const m2 = iso.match(/^(\d{4}-\d{2}-\d{2})T(\d{2})-(\d{2})/);
        if (m2) return `${m2[1]} ${m2[2]}:${m2[3]}`;
        return iso.slice(0, 16);
    }
    function fmt(v: unknown): string {
        if (v == null) return "";
        if (typeof v === "string") return v;
        try { return JSON.stringify(v, null, 2); } catch { return String(v); }
    }

    function msgText(parts: PartView[]): string {
        return parts.filter((p) => p.type === "text").map((p) => p.text).join("");
    }
    function msgThinking(parts: PartView[]): string {
        return parts.filter((p) => p.type === "thinking").map((p) => p.thinking).join("");
    }

    function toPart(p: any, fallbackId: string): PartView | null {
        if (!p || typeof p !== "object") return null;
        const type = p.type ?? "text";
        const id = p.id ?? p.partID ?? fallbackId;
        if (type === "text") {
            return { id, type, text: String(p.text ?? ""), thinking: "", tool: "", toolState: "", input: "", output: "", isError: false };
        }
        if (type === "thinking") {
            return { id, type, text: "", thinking: String(p.thinking ?? p.text ?? ""), tool: "", toolState: "", input: "", output: "", isError: false };
        }
        if (type === "tool") {
            const state = String(p.state ?? "");
            return {
                id, type, text: "", thinking: "", tool: String(p.tool ?? p.name ?? ""),
                toolState: state, input: fmt(p.input ?? p.arguments ?? p.args),
                output: fmt(p.output ?? p.result ?? ""), isError: state === "error",
            };
        }
        // snapshot / reasoning / 其他：折叠成可展开摘要，不破坏渲染
        return { id, type, text: "", thinking: "", tool: type, toolState: "", input: fmt(p), output: "", isError: false };
    }

    function renderHistory(list: unknown[]) {
        const next: MsgView[] = [];
        for (const m of list) {
            if (!m || typeof m !== "object") continue;
            const mm = m as { info?: any; parts?: unknown[] };
            const info = mm.info ?? {};
            const parts = (mm.parts ?? []).map((p, i) => toPart(p, `p-${i}`)).filter((p): p is PartView => p !== null);
            if (parts.length === 0) continue;
            const id = info.id ?? `h-${Math.random().toString(36).slice(2, 8)}`;
            next.push({
                id,
                role: info.role === "user" ? "user" : "assistant",
                parts,
                streaming: false,
            });
        }
        // 顺序由服务端决定（/message 返回的数组顺序），前端保持原样
        messages = next;
    }

    /** 事件流更新：按 messageID 找到消息，按 partID 更新 part。 */
    function upsertPart(msgId: string, part: PartView) {
        let msg = messages.find((m) => m.id === msgId);
        if (!msg) return;
        const idx = msg.parts.findIndex((p) => p.id === part.id && part.id !== "");
        if (idx >= 0) {
            const old = msg.parts[idx];
            // 流式增量：text/thinking 用服务端全量覆盖（part.updated 通常带全量）
            msg.parts[idx] = part;
        } else {
            msg.parts.push(part);
        }
    }
    function upsertMessage(info: any, parts: unknown[]) {
        if (!info?.id) return;
        const id = info.id;
        const role = info.role === "user" ? "user" : "assistant";
        const pv = (parts ?? []).map((p, i) => toPart(p, `p-${i}`)).filter((p): p is PartView => p !== null);
        let msg = messages.find((m) => m.id === id);
        if (!msg) {
            // 用户消息：本地 send() 已 push 一条，按文本去重
            if (role === "user") {
                const text = pv.filter((p) => p.type === "text").map((p) => p.text).join("");
                const last = messages[messages.length - 1];
                if (last?.role === "user" && last.parts.some((p) => p.type === "text" && p.text === text)) return;
            }
            messages.push({ id, role, parts: pv, streaming: role === "assistant" });
        } else {
            msg.role = role;
            msg.parts = pv;
            msg.streaming = role === "assistant";
        }
    }

    function handleEvent(payload: { type?: string; data?: any }) {
        const evType = payload?.type ?? "";
        const d = payload?.data;
        if (!d || typeof d !== "object") return;
        const sid = d.sessionID ?? d.properties?.sessionID ?? d.message?.sessionID;
        if (sid && currentSession && sid !== currentSession) return;
        if (evType === "message.updated" || evType === "message") {
            upsertMessage(d.info ?? d.message ?? {}, d.parts ?? d.message?.parts ?? []);
        } else if (evType === "message.part.updated" || evType === "message.part") {
            const msgId = d.messageID ?? d.message?.id ?? "";
            if (!msgId) return;
            const p = toPart(d.part ?? {}, d.partID ?? `p-${Math.random().toString(36).slice(2, 6)}`);
            if (p) upsertPart(msgId, p);
        } else if (evType === "session.idle") {
            busy = false;
        } else if (evType === "session.error" || evType === "message.error") {
            statusMsg = fmt(d.error ?? d.message ?? "opencode error");
            busy = false;
        } else if (evType === "session.start" || evType === "session.updated") {
            // 会话开始/更新：标记忙碌（新会话创建后通常跟着会话开始）
            busy = true;
        }
    }

    async function start() {
        status = "starting";
        try {
            const info = await invoke<{ id: string; local_port: number; password: string }>("opencode_session_start", { sessionId });
            ocId = info.id;
            status = "running";
            const un = await listen<{ type?: string; data?: any }>(`oc:event:${info.id}`, (e) => handleEvent(e.payload));
            unlisteners.push(un);
            // 自动建一个新会话作为当前会话
            const nid = await invoke<string>("opencode_session_new", { opencodeId: info.id });
            currentSession = nid;
            await loadHistory(nid);
        } catch (e) {
            status = "error";
            statusMsg = errMsg(e);
        }
    }

    async function loadHistory(ocSessionId: string) {
        try {
            const list = await invoke<unknown[]>("opencode_session_messages", { opencodeId: ocId, ocSessionId, limit: 60 });
            renderHistory(list);
        } catch (e) {
            statusMsg = errMsg(e);
        }
    }

    async function toggleSessionList() {
        if (showSessionList) { showSessionList = false; return; }
        if (!ocId) return;
        loadingSessions = true;
        try {
            sessionList = await invoke<OcSessionMeta[]>("opencode_session_list", { opencodeId: ocId });
            showSessionList = true;
        } catch (e) {
            statusMsg = errMsg(e);
        } finally {
            loadingSessions = false;
        }
    }

    async function switchSession(id: string) {
        showSessionList = false;
        currentSession = id;
        messages = [];
        busy = false;
        await loadHistory(id);
    }

    async function newSession() {
        showSessionList = false;
        if (!ocId) return;
        try {
            const nid = await invoke<string>("opencode_session_new", { opencodeId: ocId });
            currentSession = nid;
            messages = [];
            busy = false;
        } catch (e) {
            statusMsg = errMsg(e);
        }
    }

    async function deleteSession(id: string, ev: Event) {
        ev.stopPropagation();
        if (!ocId) return;
        if (!window.confirm(t("pi.sessions_delete_confirm"))) return;
        try {
            await invoke("opencode_session_delete", { opencodeId: ocId, ocSessionId: id });
            sessionList = sessionList.filter((s) => s.id !== id);
            if (currentSession === id) { currentSession = null; messages = []; }
        } catch (e) {
            statusMsg = errMsg(e);
        }
    }

    async function send() {
        const text = input.trim();
        if (!text || !ocId || !currentSession || status !== "running") return;
        input = "";
        messages.push({ id: `u-${Date.now()}`, role: "user", parts: [{ id: `p-${Date.now()}`, type: "text", text, thinking: "", tool: "", toolState: "", input: "", output: "", isError: false }], streaming: false });
        busy = true;
        try {
            // 同步等待完整回复（opencode 1.18 的 /message 端点，可靠）
            const reply = await invoke<{ info: any; parts: unknown[] }>("opencode_session_send", {
                opencodeId: ocId,
                ocSessionId: currentSession,
                text,
            });
            if (reply?.info?.id) {
                upsertMessage(reply.info, reply.parts ?? []);
            }
        } catch (e) {
            statusMsg = errMsg(e);
        } finally {
            busy = false;
        }
    }

    async function abort() {
        if (!ocId || !currentSession) return;
        try {
            await invoke("opencode_session_abort", { opencodeId: ocId, ocSessionId: currentSession });
        } catch { /* ignore */ }
        busy = false;
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
        void start();
    });

    onDestroy(() => {
        for (const un of unlisteners) { try { un(); } catch { /* ignore */ } }
        unlisteners = [];
        if (ocId) {
            void invoke("opencode_session_stop", { opencodeId: ocId }).catch(() => {});
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

<div class="oc-tab">
    <header class="oc-bar">
        <span class="oc-title">OpenCode</span>
        <span class="oc-status" class:on={status === "running"} class:err={status === "error"} class:busy={busy}>
            {status === "starting" ? t("pi.starting") : busy ? t("pi.working") : status === "running" ? t("pi.ready") : status === "error" ? t("pi.error") : t("pi.idle")}
        </span>
        {#if status === "error" && statusMsg}<span class="oc-status-msg">{statusMsg}</span>{/if}
        <div class="oc-bar-actions">
            <button type="button" class="oc-btn" onclick={newSession} disabled={status !== "running"}>{t("pi.new_session")}</button>
            <div class="oc-sessions-wrap">
                <button type="button" class="oc-btn" onclick={toggleSessionList} disabled={status !== "running" || loadingSessions}>
                    {t("pi.sessions")}{loadingSessions ? "…" : ""}
                </button>
                {#if showSessionList}
                    <div class="oc-sessions-drop">
                        {#if sessionList.length === 0}
                            <div class="oc-sessions-empty">{t("pi.sessions_empty")}</div>
                        {:else}
                            {#each sessionList as s (s.id)}
                                <div class="oc-session-item" class:cur={s.id === currentSession}>
                                    <button type="button" class="oc-session-open" onclick={() => void switchSession(s.id)}>
                                        <span class="oc-session-title">{s.title || t("pi.sessions_no_title")}</span>
                                        <span class="oc-session-meta">{s.directory || t("pi.sessions_unknown_cwd")} · {shortTime(s.time)}</span>
                                    </button>
                                    <button
                                        type="button"
                                        class="oc-session-del"
                                        title={t("pi.sessions_delete")}
                                        aria-label={t("pi.sessions_delete")}
                                        onclick={(e) => void deleteSession(s.id, e)}
                                    >×</button>
                                </div>
                            {/each}
                        {/if}
                    </div>
                {/if}
            </div>
            <button type="button" class="oc-btn" onclick={clearChat} disabled={messages.length === 0}>{t("pi.clear")}</button>
            <button type="button" class="oc-btn" onclick={abort} disabled={!busy}>{t("pi.abort")}</button>
        </div>
    </header>

    <div class="oc-messages" bind:this={scroller}>
        {#if messages.length === 0}
            <div class="oc-empty">
                <div class="oc-empty-title">OpenCode coding agent</div>
                <div class="oc-empty-sub">{t("pi.empty_hint")}</div>
            </div>
        {/if}
        {#each messages as msg (msg.id)}
            <div class="oc-msg" class:user={msg.role === "user"}>
                <div class="oc-msg-role">{msg.role === "user" ? t("pi.you") : "OpenCode"}</div>
                {#if msgThinking(msg.parts)}
                    <details class="oc-thinking" open={thinkingOpen}>
                        <summary>{t("pi.thinking")}</summary>
                        <pre>{msgThinking(msg.parts)}</pre>
                    </details>
                {/if}
                {#if msgText(msg.parts)}
                    <div class="oc-text">{msgText(msg.parts)}{msg.streaming ? "▍" : ""}</div>
                {/if}
                {#if msg.parts.filter((p) => p.type === "tool").length > 0}
                    <div class="oc-tools">
                        {#each msg.parts.filter((p) => p.type === "tool") as tc, ti (tc.id || `${msg.id}-${ti}`)}
                            <details class="oc-tool" class:err={tc.isError}>
                                <summary>
                                    <span class="oc-tool-name">{tc.tool || "tool"}</span>
                                    {#if tc.toolState === "running" || tc.toolState === "pending"}<span class="oc-tool-stream">…</span>{/if}
                                    {#if tc.isError}<span class="oc-tool-err">✗</span>{/if}
                                </summary>
                                {#if tc.input}<pre class="oc-tool-args">{tc.input}</pre>{/if}
                                {#if tc.output}<pre class="oc-tool-result">{tc.output}</pre>{/if}
                            </details>
                        {/each}
                    </div>
                {/if}
            </div>
        {/each}
    </div>

    <footer class="oc-input-row">
        <textarea
            class="oc-input"
            rows="2"
            placeholder={t("pi.placeholder")}
            bind:value={input}
            onkeydown={onKeydown}
        ></textarea>
        <button type="button" class="oc-send" onclick={send} disabled={!input.trim() || status !== "running" || !ocId || !currentSession}>{t("pi.send")}</button>
    </footer>
</div>

<style>
    .oc-tab {
        display: flex;
        flex-direction: column;
        height: 100%;
        background: var(--color-panel, #fafafa);
        color: var(--color-text, #222);
        font-size: 13px;
    }
    .oc-bar {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 6px 10px;
        border-bottom: 1px solid var(--color-border, #e4e4e4);
        background: var(--color-bar, #f3f3f3);
        flex: 0 0 auto;
        flex-wrap: wrap;
    }
    .oc-title { font-weight: 600; font-size: 13px; }
    .oc-status {
        font-size: 11px;
        color: var(--color-text-muted, #888);
        border: 1px solid var(--color-border, #e4e4e4);
        border-radius: 9px;
        padding: 1px 8px;
        background: #fff;
    }
    .oc-status.on { color: #2e8b57; border-color: #2e8b57; }
    .oc-status.err { color: #c0392b; border-color: #c0392b; }
    .oc-status.busy { color: #b8860b; border-color: #b8860b; }
    .oc-status-msg { font-size: 11px; color: #c0392b; max-width: 40%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .oc-bar-actions { margin-left: auto; display: flex; gap: 6px; align-items: center; }
    .oc-btn {
        font-size: 11px;
        padding: 3px 10px;
        border: 1px solid var(--color-border, #d0d0d0);
        border-radius: 6px;
        background: #fff;
        cursor: pointer;
    }
    .oc-btn:disabled { opacity: 0.5; cursor: default; }
    .oc-sessions-wrap { position: relative; }
    .oc-sessions-drop {
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
    .oc-sessions-empty { padding: 10px; font-size: 12px; color: var(--color-text-muted, #888); }
    .oc-session-item { display: flex; align-items: center; gap: 4px; border-radius: 6px; }
    .oc-session-item.cur { background: #e8f1fb; }
    .oc-session-item:hover { background: #eef2fb; }
    .oc-session-open {
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
    .oc-session-title { font-size: 12px; color: #333; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .oc-session-meta { font-family: ui-monospace, monospace; font-size: 10.5px; color: var(--color-text-muted, #888); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .oc-session-del {
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
    .oc-session-item:hover .oc-session-del { visibility: visible; }
    .oc-session-del:hover { color: #c0392b; background: rgba(192, 57, 43, 0.08); }
    .oc-messages {
        flex: 1 1 auto;
        overflow-y: auto;
        padding: 12px;
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .oc-empty { margin: auto; text-align: center; color: var(--color-text-muted, #999); }
    .oc-empty-title { font-size: 15px; font-weight: 600; margin-bottom: 4px; }
    .oc-empty-sub { font-size: 12px; }
    .oc-msg {
        display: flex;
        flex-direction: column;
        gap: 4px;
        max-width: 100%;
    }
    .oc-msg.user { align-items: flex-end; }
    .oc-msg-role { font-size: 11px; color: var(--color-text-muted, #888); }
    .oc-text {
        white-space: pre-wrap;
        word-break: break-word;
        background: #fff;
        border: 1px solid var(--color-border, #ececec);
        border-radius: 8px;
        padding: 8px 10px;
        line-height: 1.55;
        max-width: 100%;
    }
    .oc-msg.user .oc-text { background: #e8f1fb; border-color: #d4e5f7; }
    .oc-thinking {
        background: #fbf7e9;
        border: 1px solid #efe6c8;
        border-radius: 8px;
        padding: 4px 8px;
        font-size: 12px;
    }
    .oc-thinking summary { cursor: pointer; color: #8a6d3b; font-size: 11px; }
    .oc-thinking pre { white-space: pre-wrap; word-break: break-word; margin: 4px 0 0; color: #7a6a44; }
    .oc-tools { display: flex; flex-direction: column; gap: 4px; }
    .oc-tool {
        background: #f6f6f8;
        border: 1px solid #e2e2e8;
        border-radius: 8px;
        padding: 3px 8px;
        font-size: 12px;
    }
    .oc-tool.err { border-color: #e8b4b0; background: #fdf3f2; }
    .oc-tool summary { cursor: pointer; display: flex; align-items: center; gap: 6px; font-family: ui-monospace, monospace; }
    .oc-tool-name { color: #4a5aa8; font-weight: 600; }
    .oc-tool-stream { color: #b8860b; }
    .oc-tool-err { color: #c0392b; font-weight: 700; }
    .oc-tool-args, .oc-tool-result {
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
    .oc-tool-result { color: #2e5d34; }
    .oc-input-row {
        flex: 0 0 auto;
        display: flex;
        gap: 8px;
        padding: 8px 10px;
        border-top: 1px solid var(--color-border, #e4e4e4);
        background: var(--color-bar, #f3f3f3);
    }
    .oc-input {
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
    .oc-send {
        align-self: flex-end;
        padding: 8px 16px;
        border: none;
        border-radius: 8px;
        background: #3b6fd4;
        color: #fff;
        font-size: 13px;
        cursor: pointer;
    }
    .oc-send:disabled { opacity: 0.5; cursor: default; }
</style>
