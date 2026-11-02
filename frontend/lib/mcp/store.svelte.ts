import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import * as app from "../stores/app.svelte.ts";
import * as ai from "../ai/store.svelte.ts";
import type { AiSessionInfo, AiTargetKind } from "../ai/types.ts";
import { errMsg, t } from "../i18n/index.svelte.ts";

export interface AgentSession {
  session_id: string;
  tab_id: string;
  nonce: string;
  name: string;
  kind: AiTargetKind;
  shell: { kind: string; source: string };
}
export interface Endpoint { url: string; token: string }
export interface SessionRequest {
  id: string;
  session_id: string;
  tab_id: string;
  nonce: string;
  kind: AiTargetKind;
  title: string;
}
interface PendingRequest extends SessionRequest { busy: boolean; error?: string }
interface Status { endpoint: Endpoint | null; sessions: AgentSession[]; requests: SessionRequest[] }
type ConnectedSession = { sessionId: string; tabId: string; label: string; type: string };

let _endpoint = $state<Endpoint | null>(null);
let _sessions = $state<AgentSession[]>([]);
let _requests = $state<PendingRequest[]>([]);
let _error = $state("");
let revision = 0;
let mutations: Promise<unknown> = Promise.resolve();
const subscriptions = new Map<string, Promise<UnlistenFn>>();
const registrations = new Map<string, string>();
const earlyRequests = new Map<string, SessionRequest[]>();

export function endpoint() { return _endpoint; }
export function error() { return _error; }
export function requestsForTab(tabId: string) { return _requests.filter(r => r.tab_id === tabId); }

function receive(request: SessionRequest) {
  const session = _sessions.find(s => s.session_id === request.session_id && s.nonce === request.nonce);
  if (!session || app.sessionIdForTab(request.tab_id) !== request.session_id || _requests.some(r => r.id === request.id)) return;
  const target = app.tabs().find(tab => tab.id === request.tab_id);
  if (!target) return;
  _requests.push({ ...request, busy: false });
  app.setActiveWorkspace(target.workspaceId ?? target.id);
  app.setActivePane(target.id);
  app.openAiPanel(request.tab_id);
}

async function subscribe(sessionId: string) {
  if (!subscriptions.has(sessionId)) {
    subscriptions.set(sessionId, (async () => {
      const requested = await listen<SessionRequest>(`agent:session_requested:${sessionId}`, ({ payload }) => {
        if (_sessions.some(s => s.session_id === sessionId)) receive(payload);
        else earlyRequests.set(sessionId, [...(earlyRequests.get(sessionId) ?? []), payload]);
      });
      try {
        const cancelled = await listen<{ id: string }>(`agent:session_cancelled:${sessionId}`, ({ payload }) => {
          _requests = _requests.filter(r => r.id !== payload.id);
          earlyRequests.set(sessionId, (earlyRequests.get(sessionId) ?? []).filter(r => r.id !== payload.id));
        });
        return () => { requested(); cancelled(); };
      } catch (error) { requested(); throw error; }
    })());
  }
  const pending = subscriptions.get(sessionId)!;
  try { await pending; }
  catch (error) {
    if (subscriptions.get(sessionId) === pending) subscriptions.delete(sessionId);
    throw error;
  }
}

function detach(sessionId: string) {
  const pending = subscriptions.get(sessionId);
  subscriptions.delete(sessionId);
  registrations.delete(sessionId);
  earlyRequests.delete(sessionId);
  _requests = _requests.filter(r => r.session_id !== sessionId);
  _sessions = _sessions.filter(s => s.session_id !== sessionId);
  if (pending) void pending.then(stop => stop()).catch(e => { _error = errMsg(e); });
}

function mutate(action: () => Promise<void>): Promise<void> {
  const operation = mutations.then(action);
  // An unsuccessful request must not prevent later revocation or cleanup.
  mutations = operation.catch(() => {});
  return operation;
}

async function applyStatus(status: Status, expected: number) {
  if (revision !== expected) return;
  _endpoint = status.endpoint;
  _sessions = status.sessions.filter(s => app.sessionIdForTab(s.tab_id) === s.session_id);
  await Promise.all(_sessions.map(s => subscribe(s.session_id)));
  if (revision !== expected) return;
  for (const request of status.requests ?? []) receive(request);
  for (const session of _sessions) {
    for (const request of earlyRequests.get(session.session_id) ?? []) receive(request);
    earlyRequests.delete(session.session_id);
  }
}

export async function refresh() {
  const expected = revision;
  await mutate(async () => {
    if (revision === expected) await applyStatus(await invoke<Status>("mcp_status"), expected);
  });
}

/** Register terminal metadata and listeners; this does not authorize control. */
export async function reconcileSessions(connected: ConnectedSession[]) {
  const expected = revision;
  await mutate(async () => {
    if (revision !== expected) return;
    const active = new Set(connected.map(s => s.sessionId));
    for (const id of [...subscriptions.keys()]) {
      if (active.has(id)) continue;
      detach(id);
      await invoke("mcp_unregister_session", { sessionId: id });
    }
    for (const session of connected) {
      if (revision !== expected) return;
      if (app.sessionIdForTab(session.tabId) !== session.sessionId) continue;
      const key = JSON.stringify([session.tabId, session.label]);
      if (registrations.get(session.sessionId) === key) continue;
      await subscribe(session.sessionId);
      const registered = await invoke<AgentSession>("mcp_register_session", {
        sessionId: session.sessionId, tabId: session.tabId, label: session.label,
      });
      if (revision !== expected || app.sessionIdForTab(session.tabId) !== session.sessionId) {
        detach(session.sessionId);
        await invoke("mcp_unregister_session", { sessionId: session.sessionId });
        continue;
      }
      registrations.set(session.sessionId, key);
      _sessions = [..._sessions.filter(s => s.session_id !== registered.session_id), registered];
      for (const request of earlyRequests.get(session.sessionId) ?? []) receive(request);
      earlyRequests.delete(session.sessionId);
    }
  });
}

export async function start() {
  const expected = revision;
  _error = "";
  await mutate(async () => {
    if (revision === expected) await applyStatus(await invoke<Status>("mcp_start"), expected);
  });
}

export async function stop() {
  ++revision;
  _requests = [];
  _endpoint = null;
  await mutate(async () => {
    // Queue teardown with endpoint mutation so a subsequent start cannot race
    // ahead of this stop while the final conversation timeline is being saved.
    const conversations = ai.listAllSessions().filter(s => s.source === "mcp");
    const results = await Promise.allSettled(conversations.map(s => ai.endConversation(s.tab_id)));
    await invoke("mcp_stop");
    const failure = results.find(r => r.status === "rejected");
    if (failure?.status === "rejected") throw failure.reason;
  });
}

function assertRequestCurrent(request: SessionRequest) {
  if (!_requests.some(r => r.id === request.id) || app.sessionIdForTab(request.tab_id) !== request.session_id) {
    throw new Error(t("common.disconnected"));
  }
}

export async function accept(id: string) {
  const request = _requests.find(r => r.id === id);
  if (!request || request.busy) return;
  request.busy = true;
  request.error = undefined;
  let started: AiSessionInfo | undefined;
  try {
    assertRequestCurrent(request);
    const ending = ai.endConversation(request.tab_id);
    // endConversation changes the generation synchronously. Capture that
    // lifetime before awaiting its save, so closing the panel revokes this accept.
    const lease = ai.captureSessionLease(request.tab_id);
    await ending;
    assertRequestCurrent(request);
    started = await ai.startMcpSession({
      tabId: request.tab_id, targetKind: request.kind, targetId: request.session_id,
      lease,
    }, request.id);
    assertRequestCurrent(request);
    // The AI store has installed every command/audit listener before the local
    // agent is released to call run_command.
    await invoke("mcp_session_ready", { requestId: request.id, instanceId: started.instance_id });
    _requests = _requests.filter(r => r.id !== id);
  } catch (error) {
    if (started && ai.sessionForTab(started.tab_id)?.instance_id === started.instance_id) {
      try { await ai.endConversation(started.tab_id); }
      catch (cleanupError) { console.error("[mcp] close failed session:", cleanupError); }
    }
    request.error = errMsg(error);
    throw error;
  } finally { request.busy = false; }
}

export async function reject(id: string) {
  const request = _requests.find(r => r.id === id);
  if (!request || request.busy) return;
  request.busy = true;
  try {
    await invoke("mcp_reject_session", { requestId: id });
    _requests = _requests.filter(r => r.id !== id);
  } catch (error) { request.error = errMsg(error); throw error; }
  finally { request.busy = false; }
}

export function disconnect() {
  ++revision;
  for (const id of [...subscriptions.keys()]) detach(id);
  _endpoint = null;
  _requests = [];
}
