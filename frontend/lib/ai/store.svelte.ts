/**
 * AI 排障会话前端状态。
 * - 一个 tab 至多一个 AI 会话；store 全部按 tab_id 索引
 *   （切 tab / SSH 重连不丢；显式关闭 AI 面板则结束当前会话）
 * - 监听 ai:*:<tab_id> 事件填充 chat 时间线
 * - keyboard lock 在 AI 命令执行期间生效
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type Event as TauriEvent, type UnlistenFn } from "@tauri-apps/api/event";
import { saveTextFile, fileStamp } from "../save-file.ts";
import { createSidePanelState } from "../stores/panel-state.svelte.ts";
import { toast } from "../stores/toast.svelte.ts";
import { t, errMsg, locale as currentLocale } from "../i18n/index.svelte.ts";
import { truncateCommand } from "./format.ts";
import {
  applyTerminalMutations,
  restoreTimeline,
  type AiTerminalMutation,
} from "./timeline.ts";
import { sessionCommandKey, type SessionInstanceRef } from "./session-identity.ts";
import { commandApprovals, isAutoApprovalAllowed } from "./command-approval.ts";
export type { SessionInstanceRef } from "./session-identity.ts";
import type {
  AiSessionInfo,
  AiSettings,
  AiTargetKind,
  AuditLog,
  ChatItem,
  CommandProposed,
  CategoryGroup,
  CommandResult,
  ConversationMeta,
  AiProviderRecord,
  LlmProtocol,
  ModelInfo,
  RedactRuleRecord,
  ShellKind,
  SkillRecord,
  TokenUsage,
  WebToolProposal,
  WebToolResult,
  DownloadProposal,
  DownloadResult,
  AnalyzeProposal,
  AnalyzeResult,
  PatchProposal,
  MatchProposal,
} from "./types.ts";
import { isRawDeviceKind } from "./types.ts";
import { bracketedPasteEnabled } from "../terminal/bracketed-paste.ts";
import { alternateScreenActive, captureTerminalGuard, flushTerminalOutput } from "../terminal/alternate-screen.ts";

const ALTERNATE_SCREEN_ERROR = "Command was not executed: the target terminal is in alternate screen mode "
  + "(for example, Vim, less, or top). Ask the user to exit the full-screen application and return to "
  + "the normal command prompt before retrying. Do not send shell commands or exit keystrokes to that application.";

const TERMINAL_UNAVAILABLE_ERROR = "Command was not executed: the target terminal was closed or replaced, or access was revoked.";

// ─── Position ────────────────────────────────────────────────────
// 只支持 left/right。移动端用户横屏即可用——左右布局就够了，没必要再开上下分支。

export type AiPosition = "left" | "right";

const POS_KEY = "ai_panel_position";
const LEGACY_PANEL_WIDTH_KEY = "ai-panel-width";
const MIN_PANEL_WIDTH = 280;
function loadPos(): AiPosition {
  const v = localStorage.getItem(POS_KEY);
  return v === "left" || v === "right" ? v : "right";
}

// ─── Per-tab visibility ───────────────────────────────────────────

/* Per-tab panel state — the shared side-panel skeleton (open flags + widths
   + the persisted committed default that seeds new tabs). */
const panel = createSidePanelState({
  minWidth: MIN_PANEL_WIDTH,
  storageKey: LEGACY_PANEL_WIDTH_KEY,
});
let _position = $state<AiPosition>(loadPos());
let _sessionByTab = $state<Record<string, AiSessionInfo>>({});
let _draftShellByTab = $state<Record<string, ShellKind>>({});
let _shellChangeByTab = $state<Record<string, number>>({});
let _sessionStartingByTab = $state<Record<string, boolean>>({});
let _chatByTab = $state<Record<string, ChatItem[]>>({});
let _pendingByTab = $state<Record<string, CommandProposed | null>>({});
let _keyboardLockedByTab = $state<Record<string, boolean>>({});
/**
 * tab_id → cumulative token spend for the actor's lifetime. Deliberately NOT
 * reset on context clear — clearing the conversation doesn't refund tokens
 * already billed; the counter tracks money, not context size.
 */
let _tokensByTab = $state<Record<string, TokenUsage>>({});
let _settings = $state<AiSettings | null>(null);
const _targetKindByTab: Record<string, AiTargetKind> = {};

type ContextEpochState = Readonly<{
  instanceId: string;
  epoch: number;
}>;

type PendingContextChange = {
  readonly instanceId: string;
  readonly targetEpoch: number;
  readonly bufferedEvents: Array<() => void>;
};

/**
 * One monotonic conversation epoch per live actor. Both sides start at zero
 * and increment only after a context clear or rollback has been processed. Tauri
 * commands and events use independent channels, so the epoch is the fence that
 * stops an already-queued pre-clear event from rebuilding cleared UI.
 */
const _contextEpochByTab: Record<string, ContextEpochState> = {};
const _pendingContextChangeByTab: Record<string, PendingContextChange> = {};

const _unlistenersByTab: Record<string, UnlistenFn[]> = {};
const _tabGeneration: Record<string, number> = {};
const _disposedTabs = new Set<string>();
const _sessionTeardownByTab: Partial<Record<string, Promise<void>>> = {};
const _sessionLaunchesByTab: Record<string, Set<Promise<AiSessionInfo>>> = {};
type PendingConversationMutation = Readonly<{
  kind: "send";
  sequence: number;
  clientId: string;
  operation: Promise<void>;
}> | Readonly<{
  kind: "clear";
  sequence: number;
  operation: Promise<void>;
}> | Readonly<{
  kind: "shell";
  sequence: number;
  operation: Promise<void>;
}> | Readonly<{
  kind: "rollback";
  sequence: number;
  userMessageIndex: number;
  expectedUserMessages: readonly string[];
  terminalMutations: AiTerminalMutation[];
  operation: Promise<void>;
}>;
const _pendingConversationMutationsByTab: Record<
  string,
  Set<PendingConversationMutation>
> = {};
/** Backend actor actions are ordered, but independent Tauri commands are not a
 * frontend ordering primitive. Serialize messages and context changes per tab so the
 * order in which callers mutate the UI is also the order Rust receives them.
 * In particular, a clear acknowledgement and its UI reset must finish before a
 * later send can produce assistant/command events. */
const _conversationMutationTailByTab: Partial<Record<string, Promise<void>>> = {};
let _nextConversationMutationId = 0;

class SessionClosedError extends Error {
  constructor(tab_id: string) {
    super(`AI session closed for tab: ${tab_id}`);
    this.name = "SessionClosedError";
  }
}

function isAiSessionNotFound(error: unknown): boolean {
  const message = typeof error === "string"
    ? error
    : error instanceof Error ? error.message : "";
  return message.includes("ai_session_not_found");
}

function tabGeneration(tab_id: string): number {
  return _tabGeneration[tab_id] ?? 0;
}

function isTabLive(tab_id: string, generation: number): boolean {
  return !_disposedTabs.has(tab_id) && tabGeneration(tab_id) === generation;
}

function assertTabLive(tab_id: string, generation = tabGeneration(tab_id)): void {
  if (!isTabLive(tab_id, generation)) throw new SessionClosedError(tab_id);
}

/** Opaque ownership token for one panel-open conversation lifetime. Async UI
 * actions capture it before their first await; closePanel bumps generation, so
 * no old continuation can attach to or mutate a later conversation in the same tab. */
export interface SessionLease {
  readonly tabId: string;
  readonly generation: number;
}

export function captureSessionLease(tab_id: string): SessionLease {
  const generation = tabGeneration(tab_id);
  assertTabLive(tab_id, generation);
  return { tabId: tab_id, generation };
}

function generationForLease(tab_id: string, lease: SessionLease): number {
  if (lease.tabId !== tab_id) throw new SessionClosedError(tab_id);
  return lease.generation;
}

function assertLeaseLive(tab_id: string, lease: SessionLease): void {
  assertTabLive(tab_id, generationForLease(tab_id, lease));
}

export function position() { return _position; }
export function setPosition(p: AiPosition) {
  _position = p;
  localStorage.setItem(POS_KEY, p);
}

// ─── Open/close ───────────────────────────────────────────────────

export function isOpen(tab_id: string) { return panel.isOpen(tab_id); }
export function openPanel(tab_id: string) { panel.openPanel(tab_id); }
export function closePanel(tab_id: string): Promise<void> {
  panel.closePanel(tab_id);
  return endConversation(tab_id);
}

/** End this tab's conversation: archive it and reset the panel-side state
 *  (chat items, tokens, lease generation). Panel visibility is the caller's
 *  concern — closePanel hides the panel; the "new session" button keeps it
 *  open and lands straight on the initial picker state. */
export function endConversation(tab_id: string): Promise<void> {
  clearPrefill(tab_id);
  // 面板关闭即结束这轮 conversation。先换 generation，所有迟到 listener/start
  // continuation 都失效；宽度是 tab 偏好，不属于 conversation，继续保留。
  _tabGeneration[tab_id] = tabGeneration(tab_id) + 1;
  if (_sessionByTab[tab_id] || _sessionLaunchesByTab[tab_id]?.size) {
    return stopSession(tab_id);
  }
  const teardown = _sessionTeardownByTab[tab_id];
  if (teardown) return teardown;
  clearSessionState(tab_id);
  return Promise.resolve();
}
export async function togglePanel(tab_id: string): Promise<void> {
  if (panel.isOpen(tab_id)) await closePanel(tab_id);
  else panel.openPanel(tab_id);
}
export function panelWidth(tab_id: string): number | null {
  return panel.width(tab_id);
}
export function setPanelWidth(tab_id: string, width: number | null) {
  // Writes from a disposed tab's late drag continuation must not resurrect
  // state — the dispose flow owns cleanup from that point on.
  if (!_disposedTabs.has(tab_id)) panel.setWidth(tab_id, width);
}
/** Commit only at the end of a drag. The active tab keeps its own value; the
 * committed value seeds tabs created later and preserves the pre-per-tab
 * localStorage behavior across app restarts. */
export function commitPanelWidth(tab_id: string): boolean {
  if (_disposedTabs.has(tab_id)) {
    panel.clearTab(tab_id);
    return false;
  }
  return panel.commitWidth(tab_id);
}
export function discardPanelState(tab_id: string) {
  panel.clearTab(tab_id);
  clearPrefill(tab_id);
}

/** addTab 的生命周期入口；同一个 id 即使被测试/调用方复用，旧异步任务也会因
 * generation 不同而失效。正常产品路径使用 UUID，不依赖复用。 */
export function activateTab(tab_id: string) {
  _tabGeneration[tab_id] = tabGeneration(tab_id) + 1;
  delete _draftShellByTab[tab_id];
  _disposedTabs.delete(tab_id);
  panel.seedWidth(tab_id);
}

/** closeTab 的唯一 AI teardown：先同步封死后续异步 continuation，再清 UI/actor。 */
export async function disposeTab(tab_id: string): Promise<void> {
  _tabGeneration[tab_id] = tabGeneration(tab_id) + 1;
  _disposedTabs.add(tab_id);
  discardPanelState(tab_id);
  if (!_sessionByTab[tab_id] && !_sessionLaunchesByTab[tab_id]?.size) {
    clearSessionState(tab_id);
    const teardown = _sessionTeardownByTab[tab_id];
    if (teardown) await teardown;
    return;
  }
  await beginSessionTeardown(tab_id).barrier;
}

// ─── Input prefill ────────────────────────────────────────────────
// 把一段文本塞进某个 tab 的 ChatPanel 输入框（不发送）。色条"发送到 AI"用它：
// 抽块文本 → openPanel → prefillInput，让用户过目/编辑后再发。
// 每个 tab 独立一个槽；新对象 identity 保证同一段文本重复写入也能触发对应面板的 effect。
let _prefillByTab = $state<Record<string, { text: string }>>({});
export function prefillInput(tab_id: string, text: string) {
  _prefillByTab[tab_id] = { text };
}
export function pendingPrefill(tab_id: string) { return _prefillByTab[tab_id] ?? null; }
export function clearPrefill(tab_id: string) {
  delete _prefillByTab[tab_id];
}

// ─── Session ──────────────────────────────────────────────────────

export function sessionForTab(tab_id: string): AiSessionInfo | undefined {
  return _sessionByTab[tab_id];
}
export function listAllSessions(): AiSessionInfo[] {
  return Object.values(_sessionByTab);
}

/** A draft belongs to the next conversation, never to the remote connection. */
export function shellForTab(tab_id: string): ShellKind {
  const shell = _sessionByTab[tab_id]?.shell ?? _draftShellByTab[tab_id];
  return shell === "cmd" || shell === "powershell" ? shell : "posix";
}

export function shellSelectionBusy(tab_id: string): boolean {
  return _sessionStartingByTab[tab_id] === true || _shellChangeByTab[tab_id] !== undefined;
}

export async function setSessionShell(tab_id: string, shell: ShellKind, lease: SessionLease): Promise<void> {
  assertLeaseLive(tab_id, lease);
  if (shellSelectionBusy(tab_id)) throw new Error(t("error.ai_shell_change_busy"));
  const session = _sessionByTab[tab_id];
  if (!session) {
    _draftShellByTab[tab_id] = shell;
    return;
  }
  if (isRawDeviceKind(_targetKindByTab[tab_id])) throw new Error(t("error.ai_shell_change_unavailable"));
  if (shell === shellForTab(tab_id)) return;
  const sequence = ++_nextConversationMutationId;
  _shellChangeByTab[tab_id] = sequence;
  const operation = enqueueConversationMutation(tab_id, async () => {
    assertLeaseLive(tab_id, lease);
    const updated = await invoke<AiSessionInfo>("ai_session_set_shell", {
      tabId: tab_id,
      instanceId: session.instance_id,
      shell,
    });
    if (!isTabLive(tab_id, lease.generation)) return;
    const current = _sessionByTab[tab_id];
    if (!current || current.instance_id !== session.instance_id) return;
    // A reconnect may have updated target_id while this acknowledgement travelled.
    _sessionByTab[tab_id] = { ...current, shell: updated.shell };
  });
  const pending: PendingConversationMutation = { kind: "shell", sequence, operation };
  trackConversationMutation(tab_id, pending);
  try {
    await operation;
  } finally {
    untrackConversationMutation(tab_id, pending);
    if (_shellChangeByTab[tab_id] === sequence) delete _shellChangeByTab[tab_id];
  }
}

export function chatItems(tab_id: string): ChatItem[] {
  return _chatByTab[tab_id] ?? [];
}
export function pendingCommand(tab_id: string): CommandProposed | null {
  return _pendingByTab[tab_id] ?? null;
}
export function isKeyboardLocked(tab_id: string): boolean {
  return _keyboardLockedByTab[tab_id] === true;
}
export function tokenUsage(tab_id: string): TokenUsage {
  return _tokensByTab[tab_id] ?? { tokens_in: 0, tokens_out: 0 };
}

/** Sum token spend across an audit log. LlmResponse entries carry the only
 *  authoritative per-turn token counts (history doesn't), so this is the single
 *  source for rebuilding a resumed session's toolbar total. Entries with null
 *  token fields (e.g. some OpenAI streams that omit usage) contribute 0. */
export function sumAuditTokens(audit: AuditLog): TokenUsage {
  let tin = 0;
  let tout = 0;
  for (const e of audit.entries) {
    if (e.kind.type === "llm_response") {
      tin += e.kind.tokens_in ?? 0;
      tout += e.kind.tokens_out ?? 0;
    }
  }
  return { tokens_in: tin, tokens_out: tout };
}

function pushChat(tab_id: string, item: ChatItem, persist = true) {
  const arr = _chatByTab[tab_id] ?? [];
  _chatByTab[tab_id] = [...arr, item];
  if (persist) schedulePersist(tab_id);
}

function removeOptimisticUserMessage(tab_id: string, client_id: string): void {
  const items = _chatByTab[tab_id];
  if (!items) return;
  const next = items.filter(
    (item) => item.kind !== "user" || item.client_id !== client_id,
  );
  if (next.length === items.length) return;
  _chatByTab[tab_id] = next;
  schedulePersist(tab_id);
}

function timelineAfterContextChange(
  items: ChatItem[],
  sequence: number,
  rollback?: Readonly<{
    userMessageIndex: number;
    expectedUserMessages: readonly string[];
  }>,
): ChatItem[] {
  let cutoff = 0;
  if (rollback) {
    const positions = items
      .map((item, index) => item.kind === "user" ? index : -1)
      .filter((index) => index >= 0)
      .slice(0, rollback.expectedUserMessages.length);
    const matches = positions.length === rollback.expectedUserMessages.length
      && positions.every((position, index) => {
        const item = items[position];
        return item.kind === "user" && item.text === rollback.expectedUserMessages[index];
      });
    if (!matches || positions[rollback.userMessageIndex] === undefined) return items;
    cutoff = positions[rollback.userMessageIndex];
  }
  return [
    ...items.slice(0, cutoff),
    ...items.slice(cutoff).filter(
      (item) => item.kind === "user"
        && item.client_seq !== undefined
        && item.client_seq > sequence,
    ),
  ];
}

function applyContextChange(
  session: SessionInstanceRef,
  sequence: number,
  rollback?: Readonly<{
    userMessageIndex: number;
    expectedUserMessages: readonly string[];
  }>,
): void {
  if (_sessionByTab[session.tabId]?.instance_id !== session.instanceId) return;
  commandApprovals.clearSession(session);
  clearCommandExecutionsForSession(session);
  _chatByTab[session.tabId] = timelineAfterContextChange(
    _chatByTab[session.tabId] ?? [],
    sequence,
    rollback,
  );
  _pendingByTab[session.tabId] = null;
  _keyboardLockedByTab[session.tabId] = false;
  schedulePersist(session.tabId);
}

function initializeContextEpoch(session: SessionInstanceRef): void {
  _contextEpochByTab[session.tabId] = {
    instanceId: session.instanceId,
    epoch: 0,
  };
}

function beginContextChange(session: SessionInstanceRef): PendingContextChange | null {
  if (_sessionByTab[session.tabId]?.instance_id !== session.instanceId) return null;
  const current = _contextEpochByTab[session.tabId];
  const pending: PendingContextChange = {
    instanceId: session.instanceId,
    targetEpoch: current?.instanceId === session.instanceId ? current.epoch + 1 : 1,
    bufferedEvents: [],
  };
  _pendingContextChangeByTab[session.tabId] = pending;
  return pending;
}

function finishContextChange(
  session: SessionInstanceRef,
  pending: PendingContextChange | null,
  sequence: number,
  rollback?: Readonly<{
    userMessageIndex: number;
    expectedUserMessages: readonly string[];
  }>,
): void {
  // A close can invalidate the session while the clear command is awaiting its
  // processing ack. Never recreate epoch state or replay buffered callbacks for
  // that dead actor.
  if (_sessionByTab[session.tabId]?.instance_id !== session.instanceId) {
    pending?.bufferedEvents.splice(0);
    return;
  }
  const current = _contextEpochByTab[session.tabId];
  const targetEpoch = pending?.targetEpoch
    ?? (current?.instanceId === session.instanceId ? current.epoch + 1 : 1);
  _contextEpochByTab[session.tabId] = {
    instanceId: session.instanceId,
    epoch: targetEpoch,
  };
  // Epoch installation and UI reset are one synchronous transaction from the
  // event loop's point of view. Only after both are complete may epoch+1 events
  // that outran the invoke response be released.
  applyContextChange(session, sequence, rollback);
  if (pending && _pendingContextChangeByTab[session.tabId] === pending) {
    delete _pendingContextChangeByTab[session.tabId];
  }
  for (const apply of pending?.bufferedEvents.splice(0) ?? []) apply();
}

function abandonContextChange(session: SessionInstanceRef, pending: PendingContextChange | null): void {
  if (!pending) return;
  pending.bufferedEvents.splice(0);
  if (_pendingContextChangeByTab[session.tabId] === pending) {
    delete _pendingContextChangeByTab[session.tabId];
  }
}

function contextEpochFromPayload(payload: unknown): number | null | undefined {
  if (
    !payload
    || typeof payload !== "object"
    || !Object.prototype.hasOwnProperty.call(payload, "context_epoch")
  ) return undefined;
  const epoch = (payload as { context_epoch?: unknown }).context_epoch;
  return typeof epoch === "number" && Number.isSafeInteger(epoch) && epoch >= 0
    ? epoch
    : null;
}

function stripEventScope<T extends object>(payload: T): T {
  if (!("context_epoch" in payload) && !("instance_id" in payload)) return payload;
  const copy = { ...payload } as T & { context_epoch?: unknown; instance_id?: unknown };
  delete copy.context_epoch;
  delete copy.instance_id;
  return copy;
}

function belongsToSession(session: SessionInstanceRef, payload: unknown): boolean {
  if (_sessionByTab[session.tabId]?.instance_id !== session.instanceId) return false;
  return !(payload && typeof payload === "object" && "instance_id" in payload)
    || payload.instance_id === session.instanceId;
}

function acceptsContextEvent(session: SessionInstanceRef, payload: unknown): boolean {
  if (!belongsToSession(session, payload)) return false;
  const eventEpoch = contextEpochFromPayload(payload);
  // Compatibility with older backends that emitted no context_epoch field.
  if (eventEpoch === undefined) return true;
  if (eventEpoch === null) return false;
  const current = _contextEpochByTab[session.tabId];
  return current?.instanceId === session.instanceId && eventEpoch === current.epoch;
}

function dispatchContextEvent(
  session: SessionInstanceRef,
  payload: unknown,
  apply: () => void,
): void {
  if (!belongsToSession(session, payload)) return;
  const eventEpoch = contextEpochFromPayload(payload);
  if (eventEpoch === undefined) {
    // Compatibility with older backends. Without an epoch there is no safe way
    // to distinguish pre/post-clear delivery, so preserve the legacy behavior.
    apply();
    return;
  }
  if (eventEpoch === null) return;
  const current = _contextEpochByTab[session.tabId];
  if (current?.instanceId !== session.instanceId || eventEpoch < current.epoch) return;
  const pending = _pendingContextChangeByTab[session.tabId];
  if (
    pending?.instanceId === session.instanceId
    && eventEpoch >= pending.targetEpoch
  ) {
    // Rust can send the processing ack before it emits the next event, while
    // the two IPC channels can deliver them in the opposite order. Buffer only
    // the new epoch; current-epoch events may render briefly and are then
    // removed by the acknowledged clear.
    pending.bufferedEvents.push(apply);
    return;
  }
  if (eventEpoch !== current.epoch) return;
  apply();
}

function trackConversationMutation(
  tab_id: string,
  mutation: PendingConversationMutation,
): void {
  const mutations = _pendingConversationMutationsByTab[tab_id]
    ?? new Set<PendingConversationMutation>();
  _pendingConversationMutationsByTab[tab_id] = mutations;
  mutations.add(mutation);
}

function untrackConversationMutation(
  tab_id: string,
  mutation: PendingConversationMutation,
): void {
  const mutations = _pendingConversationMutationsByTab[tab_id];
  if (!mutations) return;
  mutations.delete(mutation);
  if (mutations.size === 0 && _pendingConversationMutationsByTab[tab_id] === mutations) {
    delete _pendingConversationMutationsByTab[tab_id];
  }
}

function enqueueConversationMutation(
  tab_id: string,
  run: () => Promise<void>,
): Promise<void> {
  const previous = _conversationMutationTailByTab[tab_id];
  // Start the first action synchronously so a following close cannot overtake
  // an action already initiated in the same turn. Later actions wait for the
  // prior action to settle; rejection must not poison the queue.
  const operation = previous ? previous.then(run, run) : run();
  const tail = operation.catch(() => undefined);
  _conversationMutationTailByTab[tab_id] = tail;
  void tail.then(() => {
    if (_conversationMutationTailByTab[tab_id] === tail) {
      delete _conversationMutationTailByTab[tab_id];
    }
  });
  return operation;
}

// ─── Timeline 自动保存 ─────────────────────────────────────────────
// 后端 actor 自己保存 LLM history；UI timeline 归前端管，在每个改动 chat
// 数组的事件后落库。300ms 防抖把一轮 turn 的事件突发（user_message →
// command_proposed → completed → message_end）合并成一两次写。
// fire-and-forget：持久化是旁路功能，写失败只记 console，不打断对话。

const _persistTimers: Record<string, ReturnType<typeof setTimeout>> = {};
const _persistWritesByTab: Record<string, Promise<void>> = {};

function serializeTimeline(items: ChatItem[]): string {
  // client_id/client_seq are live correlation metadata for pending mutations,
  // not conversation data. Keeping them out of storage also prevents a fresh
  // runtime's sequence counter from comparing against stale persisted values.
  return JSON.stringify(items.map((item) => item.kind === "user"
    ? { kind: "user", text: item.text, at: item.at }
    : item));
}

function queueTimelinePersist(tab_id: string, id: string, timeline: string): Promise<void> {
  // DB writes for one conversation must stay ordered. Otherwise an older slow
  // autosave can finish after the close-time flush and overwrite the final UI
  // timeline with stale content.
  const previous = _persistWritesByTab[tab_id] ?? Promise.resolve();
  const result = previous
    .then(() => invoke("ai_conversation_save_timeline", { id, timeline }))
    .then(() => undefined);
  // Ordering chains must always settle so a failed autosave cannot poison every
  // later write. Return the raw result separately: explicit close owns the final
  // snapshot and must surface its failure after the rest of teardown completes.
  const settled = result.catch((e) => {
    console.error("[ai] persist timeline:", e);
  });
  _persistWritesByTab[tab_id] = settled;
  void settled.then(() => {
    if (_persistWritesByTab[tab_id] === settled) delete _persistWritesByTab[tab_id];
  });
  return result;
}

function schedulePersist(tab_id: string) {
  if (!_sessionByTab[tab_id]) return;
  clearTimeout(_persistTimers[tab_id]);
  _persistTimers[tab_id] = setTimeout(() => {
    delete _persistTimers[tab_id];
    const id = _sessionByTab[tab_id]?.conversation_id;
    const items = _chatByTab[tab_id];
    if (!id || !items) return;
    void queueTimelinePersist(tab_id, id, serializeTimeline(items));
  }, 300);
}

/** 该 profile / 端口下持久化过的历史对话，最近活跃在前。 */
export async function listConversations(
  target_kind: AiTargetKind,
  target_id: string,
): Promise<ConversationMeta[]> {
  return invoke<ConversationMeta[]>("ai_conversations_list", {
    target: { kind: target_kind, id: target_id },
  });
}

export async function deleteConversation(id: string): Promise<void> {
  await invoke("ai_conversation_delete", { id });
}

// ─── Lifecycle ────────────────────────────────────────────────────

export interface StartSessionArgs {
  tabId: string;
  targetKind: AiTargetKind;
  targetId: string;
  skill: string;
  provider: string;
  model: string;
  lease: SessionLease;
}

export async function startSession(args: StartSessionArgs): Promise<AiSessionInfo> {
  return launchSession(args, null);
}

/** MCP uses the same actor lifecycle, listeners, timeline and command cards. */
export async function startMcpSession(
  args: Omit<StartSessionArgs, "skill" | "provider" | "model">,
  requestId: string,
): Promise<AiSessionInfo> {
  return launchSession(
    { ...args, skill: "", provider: "", model: "" },
    null,
    () => invoke<AiSessionInfo>("mcp_accept_session", { requestId, shell: shellForTab(args.tabId) }),
  );
}

/** 恢复持久化的对话：actor 带旧 history 出生，UI 灌回存储的 timeline。 */
export async function resumeSession(
  args: StartSessionArgs,
  conversationId: string,
): Promise<AiSessionInfo> {
  return launchSession(args, conversationId);
}

async function launchSession(
  args: StartSessionArgs,
  resumeId: string | null,
  start?: () => Promise<AiSessionInfo>,
): Promise<AiSessionInfo> {
  const generation = generationForLease(args.tabId, args.lease);
  assertTabLive(args.tabId, generation);
  // 面板可能刚关闭又打开。旧 actor 未 stop 完之前，同 tab_id 启新 actor
  // 会撞 session_already_exists；先过 teardown barrier，再确认期间没再次关闭。
  const teardown = _sessionTeardownByTab[args.tabId];
  if (teardown) {
    await teardown;
    assertTabLive(args.tabId, generation);
  }
  _sessionStartingByTab[args.tabId] = true;
  const launch = launchSessionAtGeneration(args, resumeId, generation, start);
  const launches = _sessionLaunchesByTab[args.tabId] ?? new Set<Promise<AiSessionInfo>>();
  _sessionLaunchesByTab[args.tabId] = launches;
  launches.add(launch);
  try {
    return await launch;
  } finally {
    launches.delete(launch);
    if (launches.size === 0 && _sessionLaunchesByTab[args.tabId] === launches) {
      delete _sessionLaunchesByTab[args.tabId];
      delete _sessionStartingByTab[args.tabId];
    }
  }
}

async function launchSessionAtGeneration(
  args: StartSessionArgs,
  resumeId: string | null,
  generation: number,
  start?: () => Promise<AiSessionInfo>,
): Promise<AiSessionInfo> {
  let info: AiSessionInfo;
  try {
    info = await (start ? start() : invoke<AiSessionInfo>("ai_session_start", {
      tabId: args.tabId,
      target: { kind: args.targetKind, id: args.targetId },
      skill: args.skill,
      provider: args.provider,
      model: args.model,
      locale: currentLocale(),
      resume: resumeId,
      shell: resumeId ? undefined : shellForTab(args.tabId),
    }));
  } catch (error) {
    // Closing can cancel a Rust Pending reservation before start returns. The
    // backend then reports reservation_lost; expose the stable frontend
    // lifecycle meaning instead of leaking that implementation detail.
    if (!isTabLive(args.tabId, generation)) throw new SessionClosedError(args.tabId);
    throw error;
  }
  if (!isTabLive(args.tabId, generation)) {
    // close 可能先打到 not_found、start 随后才成功；启动返回后必须再 stop 一次。
    await stopFailedLaunch(info, "abandoned session");
    throw new SessionClosedError(args.tabId);
  }
  try {
    // Resume must claim/activate the conversation before reading its UI blob.
    // Otherwise another tab can read a stale snapshot while the old owner is
    // closing, then win the lease later and overwrite the old owner's final UI.
    let timeline: ChatItem[] = [];
    if (resumeId) {
      const json = await invoke<string>("ai_conversation_timeline", {
        id: resumeId,
        target: { kind: args.targetKind, id: args.targetId },
      });
      assertTabLive(args.tabId, generation);
      timeline = restoreTimeline(json, t("ai.history.stale_command"));
    }
    // info.tab_id 后端权威 —— 跟 args.tabId 一定一致（后端按入参 insert），但用
    // 后端返回值就消除"未来后端 normalize tab_id"导致 cache miss 的隐患。
    _sessionByTab[info.tab_id] = info;
    _targetKindByTab[info.tab_id] = args.targetKind;
    _chatByTab[info.tab_id] = timeline;
    initializeContextEpoch({ tabId: info.tab_id, instanceId: info.instance_id });
    // Rebuild token totals BEFORE attaching listeners: the audit is still the
    // resume snapshot here (initial_audit + SessionStarted, no live events
    // yet), so sumAuditTokens can't race an assistant_message_end incrementing
    // the same counter once listeners go live.
    if (resumeId) {
      try {
        const audit = await getAudit({ tabId: info.tab_id, instanceId: info.instance_id });
        if (isTabLive(args.tabId, generation)) {
          const spent = sumAuditTokens(audit);
          if (spent.tokens_in || spent.tokens_out) {
            _tokensByTab[info.tab_id] = spent;
          }
        }
      } catch {
        // audit empty / actor raced — leave tokens at 0; non-fatal.
      }
    }
    await attachListeners(info, generation);
    assertTabLive(args.tabId, generation);
    return info;
  } catch (error) {
    clearSessionState(info.tab_id);
    await stopFailedLaunch(info, "failed launch");
    if (!isTabLive(args.tabId, generation)) throw new SessionClosedError(args.tabId);
    throw error;
  }
}

async function stopFailedLaunch(info: AiSessionInfo, label: string): Promise<void> {
  // Explicit close owns backend stop and may be holding the conversation lease
  // until its final timeline write settles. Do not release that lease early or
  // wait on the full teardown (which itself waits for this launch).
  if (_sessionTeardownByTab[info.tab_id]) return;
  await invoke("ai_session_stop", {
    tabId: info.tab_id,
    instanceId: info.instance_id,
  }).catch((e) => console.warn(`[ai] stop ${label}:`, e));
}

function clearSessionState(tab_id: string) {
  const session = _sessionByTab[tab_id];
  if (session) {
    commandApprovals.clearSession({ tabId: tab_id, instanceId: session.instance_id });
  }
  detachListeners(tab_id);
  delete _sessionByTab[tab_id];
  delete _draftShellByTab[tab_id];
  delete _shellChangeByTab[tab_id];
  delete _pendingByTab[tab_id];
  delete _keyboardLockedByTab[tab_id];
  delete _targetKindByTab[tab_id];
  delete _contextEpochByTab[tab_id];
  const pendingChange = _pendingContextChangeByTab[tab_id];
  pendingChange?.bufferedEvents.splice(0);
  delete _pendingContextChangeByTab[tab_id];
  delete _chatByTab[tab_id];
  delete _tokensByTab[tab_id];
}

export function stopSession(tab_id: string): Promise<void> {
  return beginSessionTeardown(tab_id).outcome;
}

function beginSessionTeardown(tab_id: string): {
  outcome: Promise<void>;
  barrier: Promise<void>;
} {
  const existing = _sessionTeardownByTab[tab_id];
  if (existing) return { outcome: existing, barrier: existing };
  const outcome = stopSessionNow(tab_id);
  // Coordination consumers need completion, not the initiating close's error.
  // Keeping only this always-settled promise in the map prevents an old final-
  // save failure from poisoning rapid reopen, dispose, or a later close.
  const barrier: Promise<void> = outcome.catch(() => undefined).finally(() => {
    if (_sessionTeardownByTab[tab_id] === barrier) delete _sessionTeardownByTab[tab_id];
  });
  _sessionTeardownByTab[tab_id] = barrier;
  return { outcome, barrier };
}

async function stopSessionNow(tab_id: string): Promise<void> {
  const session = _sessionByTab[tab_id];
  const launches = Array.from(_sessionLaunchesByTab[tab_id] ?? []);
  const pendingMutations = Array.from(
    _pendingConversationMutationsByTab[tab_id] ?? [],
  );

  // Always flush the final visible snapshot before clearing state. Streaming
  // deltas mutate the current bubble in place and intentionally do not restart
  // the 300ms debounce timer per token, so "no pending timer" does not mean the
  // last completed autosave is current.
  const priorPersist = _persistWritesByTab[tab_id] ?? Promise.resolve();
  if (_persistTimers[tab_id]) {
    clearTimeout(_persistTimers[tab_id]);
    delete _persistTimers[tab_id];
  }
  const items = _chatByTab[tab_id];

  // UI reset is synchronous: a rapid reopen must never flash the old session.
  // Backend termination continues below.
  clearSessionState(tab_id);

  const abandonedLaunches = launches.map((launch) =>
    launch.then(() => undefined, () => undefined),
  );

  // A session actor can be blocked waiting for the current tool outcome. Deliver
  // that outcome before asking it to shut down; raw devices abort without any
  // transport write, while shell transports retain their explicit Ctrl+C path.
  const executionEntries = Array.from(_commandExecutions.entries()).filter(
    ([, execution]) => execution.session.tabId === tab_id
      && (!session || execution.session.instanceId === session.instance_id),
  );
  const executionResults = await Promise.allSettled(
    executionEntries.map(([, entry]) => {
      if (commandExecutionStatus(entry.session, entry.commandId) === "delivery_failed"
        && entry.action.kind !== "approve") {
        return sendCommandAction(entry, { ...entry.action });
      }
      return terminateCommand(entry.session, entry.commandId);
    }),
  );
  for (const [key, execution] of executionEntries) {
    removeCommandExecution(key, execution);
  }

  // Signal-only shutdown keeps the actor and conversation lease alive. It
  // unblocks tool waits but deliberately does not join/remove the actor; pending
  // Message/context-change processing acknowledgements and the final UI timeline
  // must settle before the full stop releases ownership.
  const prepareResults = session
    ? await Promise.allSettled([
        invoke<AiTerminalMutation[]>("ai_session_prepare_stop", {
          tabId: tab_id,
          instanceId: session.instance_id,
        }),
      ])
    : [];
  const terminalMutations = prepareResults[0]?.status === "fulfilled"
    && Array.isArray(prepareResults[0].value)
    ? prepareResults[0].value
    : [];

  const mutationResults = await Promise.allSettled(
    pendingMutations.map((pending) => pending.operation),
  );
  const failedClientIds = new Set<string>();
  const successfulContextChanges: Array<Extract<PendingConversationMutation, { kind: "clear" | "rollback" }>> = [];
  mutationResults.forEach((result, index) => {
    const mutation = pendingMutations[index];
    if (result.status === "rejected" && mutation.kind === "send") {
      failedClientIds.add(mutation.clientId);
    } else if (result.status === "fulfilled" && (mutation.kind === "clear" || mutation.kind === "rollback")) {
      successfulContextChanges.push(mutation);
    }
  });
  let filteredItems = items?.filter(
    (item) => item.kind !== "user" || !item.client_id || !failedClientIds.has(item.client_id),
  );
  for (const mutation of successfulContextChanges.sort((a, b) => a.sequence - b.sequence)) {
    if (!filteredItems) break;
    if (mutation.kind === "rollback") {
      filteredItems = applyTerminalMutations(filteredItems, mutation.terminalMutations);
    }
    filteredItems = timelineAfterContextChange(
      filteredItems,
      mutation.sequence,
      mutation.kind === "rollback" ? mutation : undefined,
    );
  }
  const finalItems = filteredItems
    ? applyTerminalMutations(filteredItems, terminalMutations)
    : filteredItems;
  const persist = session?.conversation_id && finalItems
    ? queueTimelinePersist(tab_id, session.conversation_id, serializeTimeline(finalItems))
    : priorPersist;
  const [persistResult] = await Promise.allSettled([persist]);

  // Releasing the actor also releases its conversation lease. The final UI
  // timeline must be durable first; a failed write still proceeds with stop,
  // then is surfaced only to the close caller after every teardown operation.
  const stop = invoke<void>("ai_session_stop", {
    tabId: tab_id,
    ...(session ? { instanceId: session.instance_id } : {}),
  });
  // A launch can still be in pre-activation work and have no backend reservation.
  // In that ordering, not_found is expected; generation invalidation makes the
  // launch abort, while the barrier still waits for it to settle.
  const backendStop = session
    ? stop
    : stop.catch((error) => {
        if (isAiSessionNotFound(error)) return;
        throw error;
      });
  const [backendResult, launchResults] = await Promise.all([
    Promise.allSettled([backendStop]).then(([result]) => result),
    Promise.allSettled(abandonedLaunches),
  ]);
  const results: PromiseSettledResult<unknown>[] = [
    ...executionResults,
    ...prepareResults,
    ...mutationResults,
    persistResult,
    backendResult,
    ...launchResults,
  ];
  if (launches.length > 0) {
    // The first instance-less stop can miss a start that has not reserved its
    // backend slot yet. Once every captured launch settles, no such ABA window
    // remains; the frontend teardown barrier still blocks any legitimate
    // replacement, so one final tab sweep is both necessary and safe.
    const [sweepResult] = await Promise.allSettled([
      invoke<void>("ai_session_stop", { tabId: tab_id }),
    ]);
    if (sweepResult.status === "fulfilled" || !isAiSessionNotFound(sweepResult.reason)) {
      results.push(sweepResult);
    }
  }
  const failure = results.find(
    (result): result is PromiseRejectedResult => result.status === "rejected",
  );
  if (failure) throw failure.reason;
}

function sessionInstanceForLease(tab_id: string, lease: SessionLease): SessionInstanceRef {
  assertLeaseLive(tab_id, lease);
  const info = _sessionByTab[tab_id];
  if (!info) throw new SessionClosedError(tab_id);
  return { tabId: tab_id, instanceId: info.instance_id };
}

export async function sendMessage(tab_id: string, text: string, lease: SessionLease) {
  const session = sessionInstanceForLease(tab_id, lease);
  const sequence = ++_nextConversationMutationId;
  const clientId = `${session.instanceId}:${sequence}`;
  pushChat(tab_id, {
    kind: "user",
    client_id: clientId,
    client_seq: sequence,
    text,
    at: Date.now(),
  }, false);
  const operation = enqueueConversationMutation(tab_id, async () => {
    try {
      await invoke("ai_user_message", {
        tabId: session.tabId,
        instanceId: session.instanceId,
        text,
      });
      // The backend resolves only after the actor processed and persisted this
      // message. The optimistic bubble becomes durable on that processing ack.
      // Close owns its captured final snapshot, so a send that settles during
      // teardown must not recreate an autosave timer against cleared UI state.
      if (
        isTabLive(tab_id, lease.generation)
        && _sessionByTab[tab_id]?.instance_id === session.instanceId
      ) {
        schedulePersist(tab_id);
      }
    } catch (error) {
      // Exact client correlation, never text matching: equal consecutive user
      // messages are distinct, and only the rejected enqueue is rolled back.
      removeOptimisticUserMessage(tab_id, clientId);
      throw error;
    }
  });
  const pending: PendingConversationMutation = {
    kind: "send",
    sequence,
    clientId,
    operation,
  };
  trackConversationMutation(tab_id, pending);
  try {
    await operation;
  } finally {
    untrackConversationMutation(tab_id, pending);
  }
}

/** 清空 actor 的对话历史（audit log 保留）。actor 不死，下条消息从头来过。 */
export async function clearContext(tab_id: string, lease: SessionLease): Promise<void> {
  const session = sessionInstanceForLease(tab_id, lease);
  const sequence = ++_nextConversationMutationId;
  const operation = enqueueConversationMutation(tab_id, async () => {
    const pending = beginContextChange(session);
    try {
      await invoke("ai_session_clear_context", {
        tabId: session.tabId,
        instanceId: session.instanceId,
      });
      // Like user messages, the backend response is a processing ack. Frontend
      // owns the matching UI mutation. Epoch installation + reset happens
      // synchronously, then any new-epoch events that outran this response run.
      finishContextChange(session, pending, sequence);
    } catch (error) {
      abandonContextChange(session, pending);
      throw error;
    }
  });
  const pending: PendingConversationMutation = {
    kind: "clear",
    sequence,
    operation,
  };
  trackConversationMutation(tab_id, pending);
  try {
    await operation;
  } finally {
    untrackConversationMutation(tab_id, pending);
  }
}

/** Remove one user turn and everything after it from history and the UI timeline. */
export async function rollbackContext(
  tab_id: string,
  userMessageIndex: number,
  expectedText: string,
  lease: SessionLease,
): Promise<void> {
  const session = sessionInstanceForLease(tab_id, lease);
  const expectedUserMessages = (_chatByTab[tab_id] ?? [])
    .filter((item): item is Extract<ChatItem, { kind: "user" }> => item.kind === "user")
    .map((item) => item.text);
  if (
    userMessageIndex >= expectedUserMessages.length
    || expectedUserMessages[userMessageIndex] !== expectedText
  ) throw new Error("ai_context_message_mismatch");
  const sequence = ++_nextConversationMutationId;
  const terminalMutations: AiTerminalMutation[] = [];
  const operation = enqueueConversationMutation(tab_id, async () => {
    const pending = beginContextChange(session);
    try {
      const response = await invoke<AiTerminalMutation[]>("ai_session_rollback_context", {
        tabId: session.tabId,
        instanceId: session.instanceId,
        userMessageIndex,
        expectedUserMessages,
      });
      const mutations = Array.isArray(response) ? response : [];
      terminalMutations.push(...mutations);
      if (_sessionByTab[tab_id]?.instance_id === session.instanceId) {
        _chatByTab[tab_id] = applyTerminalMutations(_chatByTab[tab_id] ?? [], mutations);
      }
      finishContextChange(session, pending, sequence, {
        userMessageIndex,
        expectedUserMessages,
      });
    } catch (error) {
      abandonContextChange(session, pending);
      throw error;
    }
  });
  const pending: PendingConversationMutation = {
    kind: "rollback",
    sequence,
    userMessageIndex,
    expectedUserMessages,
    terminalMutations,
    operation,
  };
  trackConversationMutation(tab_id, pending);
  try {
    await operation;
  } finally {
    untrackConversationMutation(tab_id, pending);
  }
}

/** SSH 重连后调用：让 actor 内部把 target_id + ssh_handle 切到新 SSH 连接。 */
export async function rebindTarget(
  tab_id: string,
  target_kind: AiTargetKind,
  target_id: string,
  lease: SessionLease,
): Promise<void> {
  const generation = generationForLease(tab_id, lease);
  assertTabLive(tab_id, generation);
  const bound = _sessionByTab[tab_id];
  if (!bound) throw new SessionClosedError(tab_id);
  const conversationId = bound.conversation_id;
  await invoke("ai_session_rebind_target", {
    tabId: tab_id,
    instanceId: bound.instance_id,
    target: { kind: target_kind, id: target_id },
    conversationId,
  });
  assertTabLive(tab_id, generation);
  // 下一张命令卡的终端状态检查使用重连后的 session ID。
  const info = _sessionByTab[tab_id];
  if (
    !info
    || info.instance_id !== bound.instance_id
    || info.conversation_id !== conversationId
  ) throw new SessionClosedError(tab_id);
  _sessionByTab[tab_id] = { ...info, target_id };
}

/** 打断 actor 正在跑的 LLM 流式响应。会话上下文（history / pending command / audit）全部保留——
 *  这跟 stopSession（销毁整个会话）是两个语义。actor 不在 chat 时调用是 no-op。 */
export async function cancelStream(tab_id: string, lease: SessionLease): Promise<void> {
  const session = sessionInstanceForLease(tab_id, lease);
  await invoke("ai_cancel_stream", {
    tabId: session.tabId,
    instanceId: session.instanceId,
  });
}

/** 当前会话的助手消息是否正在流式输出 —— UI 用它把"发送"按钮切成"停止"。 */
export function isStreaming(tab_id: string): boolean {
  const arr = _chatByTab[tab_id];
  if (!arr || arr.length === 0) return false;
  const last = arr[arr.length - 1];
  return last.kind === "assistant" && last.streaming === true;
}

/** The actor owns the command, transport and result. This entry only tracks
 * the UI request until the actor acknowledges it or emits the final event. */
type CommandAction =
  | { kind: "approve"; bracketedPaste: boolean }
  | { kind: "blocked"; reason: string }
  | { kind: "stop" | "submit" };

export type CommandExecutionStatus = "running" | "reporting" | "delivery_failed" | "delivered";

type Execution = {
  session: SessionInstanceRef;
  commandId: string;
  targetId: string;
  terminalCurrent: () => boolean;
  action: CommandAction;
  pending?: Promise<void>;
  cancelPreflight?: () => void;
};

const _commandExecutions = new Map<string, Execution>();
let _commandExecutionStatusByKey = $state<Record<string, CommandExecutionStatus>>({});

export function commandExecutionStatus(session: SessionInstanceRef, id: string): CommandExecutionStatus | null {
  return _commandExecutionStatusByKey[sessionCommandKey(session, id)] ?? null;
}

export function isCommandRunning(session: SessionInstanceRef, id: string): boolean {
  return commandExecutionStatus(session, id) === "running";
}

function removeCommandExecution(key: string, expected: Execution): void {
  if (_commandExecutions.get(key) !== expected) return;
  _commandExecutions.delete(key);
  delete _commandExecutionStatusByKey[key];
  expected.cancelPreflight?.();
}

function clearCommandExecution(session: SessionInstanceRef, id: string): void {
  const key = sessionCommandKey(session, id);
  const entry = _commandExecutions.get(key);
  if (entry) removeCommandExecution(key, entry);
}

function clearCommandExecutionsForSession(session: SessionInstanceRef): void {
  for (const [key, entry] of _commandExecutions) {
    if (entry.session.tabId === session.tabId && entry.session.instanceId === session.instanceId) {
      removeCommandExecution(key, entry);
    }
  }
}

function sendCommandAction(entry: Execution, action: CommandAction): Promise<void> {
  const { session, commandId, targetId } = entry;
  const key = sessionCommandKey(session, commandId);
  // Invalidate an earlier approval that may still be draining xterm. An IPC
  // request already in flight is independently deduplicated by the actor.
  entry.cancelPreflight?.();
  entry.cancelPreflight = undefined;
  entry.action = action;
  _commandExecutionStatusByKey[key] = action.kind === "approve" ? "running" : "reporting";
  const current = () => _commandExecutions.get(key) === entry && entry.action === action;
  const operation = async () => {
    if (action.kind === "approve") {
      const paneCurrent = () => entry.terminalCurrent()
        && _sessionByTab[session.tabId]?.instance_id === session.instanceId
        && _sessionByTab[session.tabId]?.target_id === targetId;
      const flush = flushTerminalOutput(session.tabId, () => current() && paneCurrent());
      let ready = true;
      if (flush) {
        const cancelled = new Promise<boolean>(resolve => { entry.cancelPreflight = () => resolve(false); });
        ready = await Promise.race([flush, cancelled]);
      }
      if (!current()) return;
      entry.cancelPreflight = undefined;
      if (!ready || !paneCurrent()) {
        action = { kind: "blocked", reason: TERMINAL_UNAVAILABLE_ERROR };
      } else if (alternateScreenActive(session.tabId)) {
        action = { kind: "blocked", reason: ALTERNATE_SCREEN_ERROR };
      } else {
        action = { kind: "approve", bracketedPaste: bracketedPasteEnabled(session.tabId) };
      }
      entry.action = action;
    }
    const mutation = await invoke<AiTerminalMutation | null>("ai_terminal_command", {
      tabId: session.tabId, instanceId: session.instanceId, toolCallId: commandId,
      targetSessionId: targetId, action,
    });
    if (mutation && current()) {
      dispatchContextEvent(session, mutation.payload, () => {
        if (!current() || !acceptsContextEvent(session, mutation.payload)) return;
        const items = _chatByTab[session.tabId];
        if (items) _chatByTab[session.tabId] = applyTerminalMutations(items, [mutation], "live");
        if (_pendingByTab[session.tabId]?.id === commandId) {
          _pendingByTab[session.tabId] = null;
          _keyboardLockedByTab[session.tabId] = false;
        }
        commandApprovals.clear(session, commandId);
        clearCommandExecution(session, commandId);
        schedulePersist(session.tabId);
      });
    }
  };
  const pending = operation().then(() => {
    if (current()) _commandExecutionStatusByKey[key] = "delivered";
  }, error => {
    if (!current()) return;
    _commandExecutionStatusByKey[key] = "delivery_failed";
    throw error;
  });
  entry.pending = pending;
  return pending;
}

/** Approve the exact backend proposal after xterm has parsed its pending output. */
export function executeCommand(
  session: SessionInstanceRef, commandId: string, targetId: string,
  blockedReason?: string,
): Promise<void> {
  const key = sessionCommandKey(session, commandId);
  const existing = _commandExecutions.get(key);
  if (existing) {
    // Retry the same decision. In particular a failed refusal or stop cannot
    // turn into an approval when the card remounts.
    if (commandExecutionStatus(session, commandId) === "delivery_failed") {
      return sendCommandAction(existing, { ...existing.action });
    }
    return existing.pending ?? Promise.resolve();
  }
  const action: CommandAction = blockedReason
    ? { kind: "blocked", reason: blockedReason }
    : { kind: "approve", bracketedPaste: false };
  const entry: Execution = {
    session, commandId, targetId, action, terminalCurrent: captureTerminalGuard(session.tabId),
  };
  _commandExecutions.set(key, entry);
  return sendCommandAction(entry, action);
}

function finishCommand(session: SessionInstanceRef, id: string, kind: "stop" | "submit"): Promise<void> {
  const entry = _commandExecutions.get(sessionCommandKey(session, id));
  if (!entry) return Promise.resolve();
  const status = commandExecutionStatus(session, id);
  if (status === "delivered" || status === "reporting") return entry.pending ?? Promise.resolve();
  return sendCommandAction(entry, { kind });
}

export function terminateCommand(session: SessionInstanceRef, id: string): Promise<void> {
  return finishCommand(session, id, "stop");
}

export function submitCommand(session: SessionInstanceRef, id: string): Promise<void> {
  return finishCommand(session, id, "submit");
}

export async function rejectCommand(
  session: SessionInstanceRef,
  command_id: string,
  reason: string,
) {
  await invoke("ai_command_reject", {
    tabId: session.tabId,
    instanceId: session.instanceId,
    toolCallId: command_id,
    reason,
  });
}

export async function getAudit(session: SessionInstanceRef): Promise<AuditLog> {
  return invoke<AuditLog>("ai_audit_get", {
    tabId: session.tabId,
    instanceId: session.instanceId,
  });
}

export async function saveAudit(session: SessionInstanceRef, file_path: string) {
  return invoke("ai_audit_save", {
    tabId: session.tabId,
    instanceId: session.instanceId,
    filePath: file_path,
  });
}

/** 拿审计 .log 文本，用统一的 saveTextFile 存盘（桌面 / 移动 / 浏览器一套）。 */
export async function saveAuditWithDialog(session: SessionInstanceRef): Promise<string | null> {
  const text = await invoke<string>("ai_audit_log_text", {
    tabId: session.tabId,
    instanceId: session.instanceId,
  });
  return saveTextFile(text, {
    defaultName: `rssh-diagnose-${session.tabId.slice(0, 8)}-${fileStamp()}.log`,
    filters: [{ name: "Log", extensions: ["log", "txt"] }],
  });
}

// ─── Settings ─────────────────────────────────────────────────────

export function settings() { return _settings; }

type AiSettingsPatch = Partial<{
  systemPrompt: string;
  /** Active provider row id. */
  provider: string;
  dangerMode: boolean;
  autoRunCommand: boolean;
  autoMatchFile: boolean;
  autoDownloadFile: boolean;
  autoAnalyzeLocally: boolean;
  autoPatchCp: boolean;
  autoPatchModify: boolean;
  autoPatchDiff: boolean;
  autoPatchMv: boolean;
  autoWebSearch: boolean;
  autoWebFetch: boolean;
}>;

type AutoApprovalSettingKey =
  | "danger_mode"
  | "auto_run_command"
  | "auto_match_file"
  | "auto_download_file"
  | "auto_analyze_locally"
  | "auto_patch_cp"
  | "auto_patch_modify"
  | "auto_patch_diff"
  | "auto_patch_mv"
  | "auto_web_search"
  | "auto_web_fetch";

const AUTO_APPROVAL_PATCH_FIELDS = [
  ["dangerMode", "danger_mode"],
  ["autoRunCommand", "auto_run_command"],
  ["autoMatchFile", "auto_match_file"],
  ["autoDownloadFile", "auto_download_file"],
  ["autoAnalyzeLocally", "auto_analyze_locally"],
  ["autoPatchCp", "auto_patch_cp"],
  ["autoPatchModify", "auto_patch_modify"],
  ["autoPatchDiff", "auto_patch_diff"],
  ["autoPatchMv", "auto_patch_mv"],
  ["autoWebSearch", "auto_web_search"],
  ["autoWebFetch", "auto_web_fetch"],
] as const satisfies ReadonlyArray<readonly [keyof AiSettingsPatch, AutoApprovalSettingKey]>;

// A disable is a safety action, not merely a persisted preference. Keep its
// local fail-closed overlay across concurrent reloads and save failures; only a
// later explicit successful enable removes the corresponding suspension.
const _autoApprovalSuspensions = new Set<AutoApprovalSettingKey>();

function applyAutoApprovalSuspensions(settings: AiSettings): AiSettings {
  if (_autoApprovalSuspensions.size === 0) return settings;
  const effective = { ...settings };
  for (const key of _autoApprovalSuspensions) effective[key] = false;
  return effective;
}

function installGlobalSettings(snapshot: AiSettings): AiSettings {
  const effective = applyAutoApprovalSuspensions(snapshot);
  _settings = effective;
  revokeDisallowedCommandApprovals(effective);
  return effective;
}

function suspendDisabledAutoApprovals(patch: AiSettingsPatch): void {
  let changed = false;
  for (const [patchKey, settingsKey] of AUTO_APPROVAL_PATCH_FIELDS) {
    if (patch[patchKey] === false) {
      _autoApprovalSuspensions.add(settingsKey);
      changed = true;
    }
  }
  if (changed && _settings) installGlobalSettings(_settings);
}

function releaseEnabledAutoApprovals(patch: AiSettingsPatch): void {
  for (const [patchKey, settingsKey] of AUTO_APPROVAL_PATCH_FIELDS) {
    if (patch[patchKey] === true) _autoApprovalSuspensions.delete(settingsKey);
  }
}

/** The auto-approval kind key for a tool card, or null for non-tool items.
 *  Covers every card type so a danger-mode toggle revokes pending approvals
 *  across the whole chat, not just run_command cards. */
function cardAutoApprovalKind(item: ChatItem): string | null {
  switch (item.kind) {
    case "command": return item.cmd.kind ?? "run_command";
    case "web_tool": return item.proposal.kind;
    case "patch": return `patch_${item.proposal.step}`;
    case "match": return "match_file";
    case "download": return "download_file";
    case "analyze": return "analyze_locally";
    default: return null;
  }
}

function cardId(item: ChatItem): string | null {
  switch (item.kind) {
    case "command": return item.cmd.id;
    case "web_tool":
    case "patch":
    case "match":
    case "download":
    case "analyze":
      return item.proposal.id;
    default: return null;
  }
}

function revokeDisallowedCommandApprovals(settings: AiSettings): void {
  for (const [tabId, session] of Object.entries(_sessionByTab)) {
    for (const item of _chatByTab[tabId] ?? []) {
      // Only tool cards (run_command + the proposal cards) carry pending
      // approvals worth revoking; skip plain bubbles and settled cards.
      if (item.kind !== "command" && !("proposal" in item)) continue;
      if (item.result || item.rejected) continue;
      const kind = cardAutoApprovalKind(item);
      const id = cardId(item);
      if (kind && id && !isAutoApprovalAllowed(settings, kind)) {
        commandApprovals.revokeEligibility(
          { tabId, instanceId: session.instance_id },
          id,
        );
      }
    }
  }
}

/**
 * provider 为空 → 拉 active provider 的快照，**更新**全局 `_settings`（ChatPanel 起 session 读它）；
 * provider 非空 → 仅返回该 provider 行的快照，**不动**全局缓存。
 */
export async function loadSettings(provider?: string): Promise<AiSettings> {
  const snapshot = await invoke<AiSettings>("ai_settings_get", { provider: provider || null });
  if (!provider) {
    // Commands can be hidden behind AuditPanel and have no mounted dialog to
    // observe a settings transition. Revocation therefore lives with the
    // global settings snapshot; enabling never grants an existing false entry.
    return installGlobalSettings(snapshot);
  }
  return snapshot;
}
export async function saveSettings(s: AiSettingsPatch) {
  // Disable takes effect before the first await. A command arriving while the
  // DB write is pending must see the safe policy; failure deliberately leaves
  // the suspension in place until an explicit enable succeeds.
  suspendDisabledAutoApprovals(s);
  // Backend takes a single `patch` object (AiSettingsPatch) — every field is
  // "update if present". Wrap the partial settings accordingly.
  await invoke("ai_settings_set", { patch: s });
  const snapshot = await invoke<AiSettings>("ai_settings_get", { provider: null });
  releaseEnabledAutoApprovals(s);
  installGlobalSettings(snapshot);
}

// ─── Provider 实体（设置页列表/表单）──────────────────────────────

export async function loadProviders(): Promise<AiProviderRecord[]> {
  return invoke<AiProviderRecord[]>("ai_provider_list");
}

/** Subscription credentials and API keys both enable AI on this device. */
export function hasCredentials(snapshot: { has_credentials?: boolean; has_api_key: boolean } | null | undefined): boolean {
  return snapshot?.has_credentials ?? snapshot?.has_api_key ?? false;
}

export interface ProviderSaveInput {
  /** 空 = 新建（后端生成 id）。 */
  id?: string;
  name: string;
  protocol: LlmProtocol;
  model: string;
  endpoint: string;
  /** 空串=删除已存 key；undefined=保留。 */
  apiKey?: string;
  activate?: boolean;
}

/** 新建/更新一行，返回行 id。 */
export async function saveProvider(p: ProviderSaveInput): Promise<string> {
  const id = await invoke<string>("ai_provider_save", { patch: p });
  // New sessions read the cached active provider's model and credentials.
  if (_settings?.provider === id || p.activate) await loadSettings();
  return id;
}

export async function deleteProvider(id: string): Promise<void> {
  await invoke("ai_provider_delete", { id });
}

/** 设为当前 provider（刷新全局 settings 快照）。 */
export async function activateProvider(id: string): Promise<void> {
  await saveSettings({ provider: id });
}

/**
 * 拉模型列表（表单"拉取"按钮）。protocol+endpoint 用表单当前值（未保存也能试）；
 * apiKey 缺省时后端按 providerId 从 secret_store 取已存值。
 * 服务端不开放 /models（如智谱）会报错 —— 下拉留空即可。
 */
export async function listModels(
  protocol: LlmProtocol,
  endpoint: string,
  opts?: { providerId?: string; apiKey?: string },
): Promise<ModelInfo[]> {
  return invoke<ModelInfo[]>("ai_list_models", {
    providerId: opts?.providerId || null,
    protocol,
    endpoint,
    apiKey: opts?.apiKey || null,
  });
}

// ─── 事件监听 ─────────────────────────────────────────────────────

async function attachListeners(info: AiSessionInfo, generation: number) {
  // tab_id 同时是状态字典 key、事件 topic 后缀、internal_command 路由 key —— 单一坐标。
  // info.target_id 不在闭包里捕获 —— 重连后 target_id 变了，internal_command 需要走新的，
  // 闭包里写死会一直发到旧 SSH 会话。运行期通过 _sessionByTab[tab_id].target_id 读最新值。
  const tab = info.tab_id;
  const session = { tabId: tab, instanceId: info.instance_id };
  const u: UnlistenFn[] = [];
  assertTabLive(tab, generation);
  detachListeners(tab);
  // 先登记正在构建的数组。dispose 若发生在任一个 listen await 中，可以立刻
  // 清掉已完成的 listener；迟到的 listener 由下面 catch 再清一次。
  _unlistenersByTab[tab] = u;

  const addListener = async <T>(
    event: string,
    handler: (event: TauriEvent<T>) => void | Promise<void>,
    beforeContextGate?: (event: TauriEvent<T>) => void,
  ) => {
    const unlisten = await listen<T>(event, (payload) => {
      // unlisten 不能撤回已排队的 callback；generation gate 防止关闭后的
      // 迟到事件重新写入 chat/pending/keyboard 状态。context epoch gate
      // separately rejects callbacks queued before a successful context clear.
      if (!isTabLive(tab, generation)) return;
      if (!belongsToSession(session, payload.payload)) return;
      beforeContextGate?.(payload);
      dispatchContextEvent(session, payload.payload, () => {
        if (
          isTabLive(tab, generation)
          && acceptsContextEvent(session, payload.payload)
        ) {
          void handler(payload);
        }
      });
    });
    if (!isTabLive(tab, generation)) {
      unlisten();
      throw new SessionClosedError(tab);
    }
    u.push(unlisten);
  };

  try {
  // 流式：start 创建空气泡，delta append，end 关 streaming 标记
  await addListener<{ id: string }>(`ai:assistant_message_start:${tab}`, (e) => {
    pushChat(tab, { kind: "assistant", id: e.payload.id, text: "", at: Date.now(), streaming: true });
  });

  await addListener<{ id: string; text: string }>(`ai:assistant_delta:${tab}`, (e) => {
    const arr = _chatByTab[tab];
    if (!arr) return;
    // Mutate the matching item in place. Svelte 5's $state proxy picks up
    // field assignments, so we don't need React-style full-array rebuilds
    // (which were O(N) per token — an 8 000-token streamed reply over a
    // 100-message chat is 800 000 array clones / 24 MB of GC churn).
    for (let i = arr.length - 1; i >= 0; i--) {
      const item = arr[i];
      if (item.kind === "assistant" && item.id === e.payload.id) {
        item.text += e.payload.text;
        return;
      }
    }
  });

  await addListener<{
    id: string; text: string; cancelled?: boolean;
    tokens_in?: number | null; tokens_out?: number | null;
  }>(`ai:assistant_message_end:${tab}`, (e) => {
    const arr = _chatByTab[tab] ?? [];
    for (let i = arr.length - 1; i >= 0; i--) {
      const item = arr[i];
      if (item.kind === "assistant" && item.id === e.payload.id) {
        const isEmpty = !e.payload.text || e.payload.text.length === 0;
        // cancelled=true 时即使 text 空也要保留气泡——UI 模板会渲染本地化的
        // "已停止"徽章，告诉用户这一轮被自己打断了。
        // 只有"纯 tool_use 轮次"（chat 没产文本只产 tool_calls，cancelled=false）
        // 或 chat 失败（empty + cancelled=false）才移除气泡。
        if (isEmpty && !e.payload.cancelled) {
          _chatByTab[tab] = [...arr.slice(0, i), ...arr.slice(i + 1)];
        } else {
          // 防御：cancel emit 的 payload.text = 后端 captured（sink 累积）；前端 item.text =
          // 收到的 delta 累积。两者源头一致，正常情况下相等。但 tauri 事件总线异步——
          // cancel emit 抵达时若 in-flight delta 尚未处理完，payload 反而可能比 item.text
          // 短；极端退化时甚至为空（chat 刚 start 就 cancel）。用 item.text 兜底，
          // 避免"用户看着字一行行出来，按停止后只剩个徽章"。
          const finalText = e.payload.text || item.text;
          const replaced: ChatItem = {
            ...item,
            text: finalText,
            streaming: false,
            cancelled: e.payload.cancelled === true,
          };
          _chatByTab[tab] = [...arr.slice(0, i), replaced, ...arr.slice(i + 1)];
        }
        schedulePersist(tab);
        return;
      }
    }
  }, (e) => {
    // Billing belongs to the actor lifetime, not the visible context. Account
    // it before the context gate so a delayed pre-clear terminal event cannot
    // rebuild its bubble but still contributes its already-spent tokens once.
    // Pure tool_use turns (empty text) are billed too; cancelled streams carry
    // no token fields and therefore add nothing.
    const tin = e.payload.tokens_in ?? 0;
    const tout = e.payload.tokens_out ?? 0;
    if (tin > 0 || tout > 0) {
      const cur = _tokensByTab[tab] ?? { tokens_in: 0, tokens_out: 0 };
      _tokensByTab[tab] = { tokens_in: cur.tokens_in + tin, tokens_out: cur.tokens_out + tout };
    }
  });

  await addListener<CommandProposed>(`ai:command_proposed:${tab}`, (e) => {
    const proposed = stripEventScope(e.payload);
    // Authorization belongs to command arrival, not component mount. The chat
    // list is unmounted while AuditPanel is visible; reading live settings when
    // it later remounts would let a subsequent enable retro-authorize this cmd.
    commandApprovals.snapshotEligibility(
      { tabId: tab, instanceId: info.instance_id },
      proposed.id,
      isAutoApprovalAllowed(_settings, proposed.kind),
    );
    _pendingByTab[tab] = proposed;
    pushChat(tab, { kind: "command", cmd: proposed, at: Date.now() });
    const targetId = proposed.target_session_id ?? _sessionByTab[tab]?.target_id;
    if (proposed.kind === "run_command" && targetId) {
      void (async () => {
        const output = flushTerminalOutput(tab, () =>
          isTabLive(tab, generation) && _pendingByTab[tab]?.id === proposed.id);
        if (output && !await output) return;
        // The approval/card or actor can finish while the output is parsing.
        if (!isTabLive(tab, generation)
          || _sessionByTab[tab]?.instance_id !== session.instanceId
          || _pendingByTab[tab]?.id !== proposed.id
          || !acceptsContextEvent(session, e.payload)
          || !alternateScreenActive(tab)) return;
        commandApprovals.markAttempted(session, proposed.id);
        await executeCommand(session, proposed.id, targetId, ALTERNATE_SCREEN_ERROR);
      })().catch((error) => console.error("[ai] alternate-screen refusal delivery failed:", error));
    }
  });

  await addListener<WebToolProposal>(`ai:web_tool_proposed:${tab}`, (e) => {
    const proposal = stripEventScope(e.payload);
    commandApprovals.snapshotEligibility(
      { tabId: tab, instanceId: info.instance_id },
      proposal.id,
      isAutoApprovalAllowed(_settings, proposal.kind),
    );
    pushChat(tab, { kind: "web_tool", proposal, at: Date.now() });
  });

  await addListener<DownloadProposal>(`ai:download_proposed:${tab}`, (e) => {
    const proposal = stripEventScope(e.payload);
    commandApprovals.snapshotEligibility(
      { tabId: tab, instanceId: info.instance_id },
      proposal.id,
      isAutoApprovalAllowed(_settings, "download_file"),
    );
    pushChat(tab, { kind: "download", proposal, at: Date.now() });
  });

  await addListener<AnalyzeProposal>(`ai:analyze_proposed:${tab}`, (e) => {
    const proposal = stripEventScope(e.payload);
    commandApprovals.snapshotEligibility(
      { tabId: tab, instanceId: info.instance_id },
      proposal.id,
      isAutoApprovalAllowed(_settings, "analyze_locally"),
    );
    pushChat(tab, { kind: "analyze", proposal, at: Date.now() });
  });

  await addListener<PatchProposal>(`ai:patch_proposed:${tab}`, (e) => {
    const proposal = stripEventScope(e.payload);
    // Per-step auto-approval: patch_cp / patch_modify / patch_diff / patch_mv
    // each map to their own settings.auto_patch_<step> toggle.
    commandApprovals.snapshotEligibility(
      { tabId: tab, instanceId: info.instance_id },
      proposal.id,
      isAutoApprovalAllowed(_settings, `patch_${proposal.step}`),
    );
    pushChat(tab, { kind: "patch", proposal, at: Date.now() });
  });

  await addListener<MatchProposal>(`ai:match_proposed:${tab}`, (e) => {
    const proposal = stripEventScope(e.payload);
    commandApprovals.snapshotEligibility(
      { tabId: tab, instanceId: info.instance_id },
      proposal.id,
      isAutoApprovalAllowed(_settings, "match_file"),
    );
    pushChat(tab, { kind: "match", proposal, at: Date.now() });
  });

  // Capability probes use the same xterm checks as visible cards. The actor
  // acknowledges completion after consuming the result, including for probes.
  await addListener<{ id: string; target_session_id?: string }>(`ai:internal_command:${tab}`, async (e) => {
    const id = e.payload.id;
    const targetId = e.payload.target_session_id ?? _sessionByTab[tab]?.target_id;
    if (!targetId) return;
    try {
      await executeCommand(session, id, targetId);
    } catch {
      // A lost response can be retried by card ID without replaying the command.
      try {
        await executeCommand(session, id, targetId);
      } catch (error) {
        console.error("[ai] internal command acknowledgement failed:", error);
        return;
      }
    }
    clearCommandExecution(session, id);
  });

  await addListener<{ id: string; lock_keyboard: boolean }>(`ai:command_executing:${tab}`, (e) => {
    _keyboardLockedByTab[tab] = !!e.payload.lock_keyboard;
  });

  await addListener<CommandResult & { lock_keyboard: boolean }>(`ai:command_completed:${tab}`, (e) => {
    if (!_pendingByTab[tab] || _pendingByTab[tab]?.id === e.payload.id) {
      _keyboardLockedByTab[tab] = !!e.payload.lock_keyboard;
      _pendingByTab[tab] = null;
    }
    // 给最近一条对应 id 的 command 项填上 result
    const arr = _chatByTab[tab] ?? [];
    for (let i = arr.length - 1; i >= 0; i--) {
      const item = arr[i];
      if (item.kind === "command" && item.cmd.id === e.payload.id) {
        clearCommandExecution(
          { tabId: tab, instanceId: info.instance_id },
          e.payload.id,
        );
        commandApprovals.clear(
          { tabId: tab, instanceId: info.instance_id },
          e.payload.id,
        );
        const replaced: ChatItem = { ...item, result: stripEventScope(e.payload) };
        _chatByTab[tab] = [...arr.slice(0, i), replaced, ...arr.slice(i + 1)];
        schedulePersist(tab);
        break;
      }
    }
  });

  await addListener<WebToolResult>(`ai:web_tool_completed:${tab}`, (e) => {
    const result = stripEventScope(e.payload);
    const arr = _chatByTab[tab] ?? [];
    for (let i = arr.length - 1; i >= 0; i--) {
      const item = arr[i];
      if (item.kind === "web_tool" && item.proposal.id === result.id) {
        commandApprovals.clear(
          { tabId: tab, instanceId: info.instance_id },
          result.id,
        );
        const replaced: ChatItem = { ...item, result };
        _chatByTab[tab] = [...arr.slice(0, i), replaced, ...arr.slice(i + 1)];
        schedulePersist(tab);
        break;
      }
    }
  });

  await addListener<DownloadResult>(`ai:download_completed:${tab}`, (e) => {
    const result = stripEventScope(e.payload);
    const arr = _chatByTab[tab] ?? [];
    for (let i = arr.length - 1; i >= 0; i--) {
      const item = arr[i];
      if (item.kind === "download" && item.proposal.id === result.id) {
        commandApprovals.clear(
          { tabId: tab, instanceId: info.instance_id },
          result.id,
        );
        const replaced: ChatItem = { ...item, result };
        _chatByTab[tab] = [...arr.slice(0, i), replaced, ...arr.slice(i + 1)];
        schedulePersist(tab);
        break;
      }
    }
  });

  await addListener<AnalyzeResult>(`ai:analyze_completed:${tab}`, (e) => {
    const result = stripEventScope(e.payload);
    const arr = _chatByTab[tab] ?? [];
    for (let i = arr.length - 1; i >= 0; i--) {
      const item = arr[i];
      if (item.kind === "analyze" && item.proposal.id === result.id) {
        commandApprovals.clear(
          { tabId: tab, instanceId: info.instance_id },
          result.id,
        );
        const replaced: ChatItem = { ...item, result };
        _chatByTab[tab] = [...arr.slice(0, i), replaced, ...arr.slice(i + 1)];
        schedulePersist(tab);
        break;
      }
    }
  });

  // patch_completed carries the same PTY result shape as command_completed;
  // patch cards reuse executeCommand (hence clearCommandExecution), so the
  // registry cleanup mirrors the command path.
  await addListener<CommandResult>(`ai:patch_completed:${tab}`, (e) => {
    const result = stripEventScope(e.payload);
    const arr = _chatByTab[tab] ?? [];
    for (let i = arr.length - 1; i >= 0; i--) {
      const item = arr[i];
      if (item.kind === "patch" && item.proposal.id === result.id) {
        clearCommandExecution(
          { tabId: tab, instanceId: info.instance_id },
          result.id,
        );
        commandApprovals.clear(
          { tabId: tab, instanceId: info.instance_id },
          result.id,
        );
        const replaced: ChatItem = { ...item, result };
        _chatByTab[tab] = [...arr.slice(0, i), replaced, ...arr.slice(i + 1)];
        schedulePersist(tab);
        break;
      }
    }
  });

  // match_completed: same PTY result shape; match cards reuse executeCommand.
  await addListener<CommandResult>(`ai:match_completed:${tab}`, (e) => {
    const result = stripEventScope(e.payload);
    const arr = _chatByTab[tab] ?? [];
    for (let i = arr.length - 1; i >= 0; i--) {
      const item = arr[i];
      if (item.kind === "match" && item.proposal.id === result.id) {
        clearCommandExecution(
          { tabId: tab, instanceId: info.instance_id },
          result.id,
        );
        commandApprovals.clear(
          { tabId: tab, instanceId: info.instance_id },
          result.id,
        );
        const replaced: ChatItem = { ...item, result };
        _chatByTab[tab] = [...arr.slice(0, i), replaced, ...arr.slice(i + 1)];
        schedulePersist(tab);
        break;
      }
    }
  });

  // 拒绝路径单独事件 —— complete 跟 reject 是两种语义，复用 command_completed
  // 加 rejected:true 字段会让 listener 分支模糊。后端 RejectCommand 分支 emit
  // 这个，前端清 pending + 标记 ChatItem.rejected。所有按 id 匹配的卡片
  // （command / web_tool / download / analyze / patch / match）共用这条 reject 路径。
  await addListener<{ id: string; reason: string }>(`ai:command_rejected:${tab}`, (e) => {
    if (_pendingByTab[tab]?.id === e.payload.id) _pendingByTab[tab] = null;
    const arr = _chatByTab[tab] ?? [];
    for (let i = arr.length - 1; i >= 0; i--) {
      const item = arr[i];
      if (item.kind !== "command" && item.kind !== "web_tool" && item.kind !== "download" && item.kind !== "analyze" && item.kind !== "patch" && item.kind !== "match") continue;
      const itemId = item.kind === "command" ? item.cmd.id : item.proposal.id;
      if (itemId === e.payload.id) {
        clearCommandExecution(
          { tabId: tab, instanceId: info.instance_id },
          e.payload.id,
        );
        commandApprovals.clear(
          { tabId: tab, instanceId: info.instance_id },
          e.payload.id,
        );
        const replaced: ChatItem = { ...item, rejected: { reason: e.payload.reason } };
        _chatByTab[tab] = [...arr.slice(0, i), replaced, ...arr.slice(i + 1)];
        schedulePersist(tab);
        break;
      }
    }
  });

  // load_skill 成功 —— 后端 emit 这个，聊天面板出一条低调的 note 气泡，告诉用户
  // AI 加载了哪个用户技能（read-only，无审批卡片）。审计日志另有 SkillLoaded 条目。
  await addListener<{ id: string; name: string }>(`ai:skill_loaded:${tab}`, (e) => {
    pushChat(tab, { kind: "note", text: t("ai.note.skill_loaded", { name: e.payload.name }), at: Date.now() });
  });

  // rssh 黑名单直接拦掉命令（命令从未变成审批卡片）—— 后端 emit 这个，否则安全层
  // 静默触发，用户只看到 AI 莫名改了方案。命令可能很长，截断显示。
  await addListener<{ cmd: string; reason: string }>(`ai:command_blocked:${tab}`, (e) => {
    pushChat(tab, { kind: "note", text: t("ai.note.command_blocked", { cmd: truncateCommand(e.payload.cmd), reason: e.payload.reason }), at: Date.now() });
  });

  await addListener<{ message: string }>(`ai:error:${tab}`, (e) => {
    pushChat(tab, { kind: "error", text: e.payload.message, at: Date.now() });
  });

  await addListener<{}>(`ai:session_ended:${tab}`, () => {
    pushChat(tab, { kind: "note", text: t("ai.session.ended_note"), at: Date.now() });
    if (info.source === "mcp") {
      // External MCP stop only revokes access and signals the actor. Reuse the
      // normal teardown to drain terminal execution, save the final timeline,
      // and release this exact instance's backend conversation lease.
      clearCommandExecutionsForSession(session);
      void endConversation(tab).catch(error => toast.error(errMsg(error)));
    }
  });

    assertTabLive(tab, generation);
  } catch (error) {
    const listeners = u.splice(0);
    listeners.forEach((fn) => fn());
    if (_unlistenersByTab[tab] === u) delete _unlistenersByTab[tab];
    throw error;
  }
}

function detachListeners(tab_id: string) {
  const arr = _unlistenersByTab[tab_id];
  if (arr) {
    delete _unlistenersByTab[tab_id];
    const listeners = arr.splice(0);
    listeners.forEach(fn => fn());
  }
}

// ─── Skill 管理 ────────────────────────────────────────────────────

export async function listSkills(): Promise<SkillRecord[]> {
  return invoke<SkillRecord[]>("ai_list_skills");
}

export async function getSkill(id: string): Promise<SkillRecord | null> {
  return invoke<SkillRecord | null>("ai_get_skill", { id });
}

export async function saveSkill(s: { id: string; name: string; description: string; content: string }): Promise<void> {
  return invoke("ai_save_skill", s);
}

export async function deleteSkill(id: string): Promise<void> {
  return invoke("ai_delete_skill", { id });
}

// ─── 脱敏规则管理 ──────────────────────────────────────────────────
// 变更只对新会话生效（后端建会话时 snapshot）。saveRedactRule 在后端编译校验正则，
// 坏正则会抛 redact_invalid_regex。

export async function listRedactRules(): Promise<RedactRuleRecord[]> {
  return invoke<RedactRuleRecord[]>("ai_list_redact_rules");
}

export async function saveRedactRule(r: { id: string; pattern: string; replacement: string }): Promise<void> {
  return invoke("ai_save_redact_rule", r);
}

export async function deleteRedactRule(id: string): Promise<void> {
  return invoke("ai_delete_redact_rule", { id });
}

// ─── 命令黑名单管理 ──────────────────────────────────────────────────
// 整类编辑：保存即整类替换。改动只对新会话生效（后端建会话时 snapshot）。
// replaceCommandBlacklist 在后端校验命令名，坏名会抛 blacklist_invalid_name。

export async function listCommandBlacklist(): Promise<CategoryGroup[]> {
  return invoke<CategoryGroup[]>("ai_list_command_blacklist");
}

export async function replaceCommandBlacklist(category: string, names: string[]): Promise<void> {
  return invoke("ai_replace_command_blacklist", { category, names });
}
