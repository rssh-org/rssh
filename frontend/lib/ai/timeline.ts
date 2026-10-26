/**
 * Persisted-timeline restore. The stored blob is whatever ChatItem[] looked
 * like at the last autosave — written by a live session, so it can contain
 * live-only states that must not be resurrected verbatim:
 *
 * - assistant `streaming: true` → a blinking cursor with no stream behind it
 * - command / web_tool cards with neither result nor rejection → approval
 *   buttons whose ack the restarted backend has never heard of
 *
 * Both are normalized here. Unknown/corrupt entries are dropped, not thrown:
 * a damaged blob should degrade to a shorter timeline, never block resume.
 */
import type { ChatItem, DownloadResult, AnalyzeResult, PtyExecution } from "./types.ts";

export interface AiTerminalMutation {
  kind: string;
  payload: unknown;
}

function isStr(v: unknown): v is string {
  return typeof v === "string";
}

/** Guarantee a PTY card's `execution` envelope exists, migrating the
 *  pre-PtyExecution-split flat shape (full_cmd / sentinel / timeout_s at the
 *  top level) if that's what the blob holds. CommandConfirmDialog /
 *  PatchConfirmCard / MatchConfirmCard all dereference execution.timeout_s, so
 *  without this a card of any lineage crashes the panel on resume. */
function ensureExecution(obj: Record<string, unknown>): PtyExecution {
  const ex = obj.execution;
  if (ex && typeof ex === "object" && !Array.isArray(ex)) {
    return ex as PtyExecution;
  }
  return {
    full_cmd: isStr(obj.full_cmd) ? obj.full_cmd : "",
    sentinel: isStr(obj.sentinel) ? obj.sentinel : "",
    timeout_s: typeof obj.timeout_s === "number" ? obj.timeout_s : 0,
  };
}

/** Structural check only — is this entry a card at all? A known kind, a numeric
 *  timestamp, and the primary payload object the template dereferences (cmd for
 *  command, proposal for the rest; text for bubbles). Missing domain FIELDS are
 *  not validated here: they're normalized in restoreTimeline and render as-is
 *  (empty shows empty).
 *
 *  This replaces the old per-field validator. Validation was fragile — it had
 *  to mirror every template dereference exactly, and the single one it missed
 *  (cmd.execution) crashed the whole panel. Normalization makes that class of
 *  bug impossible: a missed default renders blank instead of throwing. */
function isCard(item: ChatItem): boolean {
  if (typeof item.at !== "number") return false;
  switch (item.kind) {
    case "user":
    case "error":
    case "note":
      return isStr(item.text);
    case "assistant":
      return isStr(item.id) && isStr(item.text);
    case "command":
      return !!item.cmd && typeof item.cmd === "object";
    case "patch":
    case "match":
    case "download":
    case "analyze":
    case "web_tool":
      return !!item.proposal && typeof item.proposal === "object";
    default:
      return false;
  }
}

export function restoreTimeline(json: string, staleCommandReason: string): ChatItem[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];

  const items: ChatItem[] = [];
  for (const raw of parsed) {
    if (!raw || typeof raw !== "object") continue;
    const item = raw as ChatItem;
    if (!isCard(item)) continue;
    if (item.kind === "user") {
      // client_id/client_seq only correlate optimistic mutations in the live
      // renderer. Old versions persisted them, but carrying those sequence
      // numbers into a new process would make a later context clear compare
      // against a counter from the wrong runtime.
      items.push({ kind: "user", text: item.text, at: item.at });
      continue;
    }
    if (item.kind === "assistant") {
      // Mirror the live assistant_message_end rule: an empty non-cancelled
      // bubble (the placeholder pushed at message_start, persisted by a
      // mid-stream crash, or a pure tool-use turn) is removed there — restore
      // must drop it too, or it renders as a permanent "…".
      if (!item.text && !item.cancelled) continue;
      item.streaming = false;
    } else if (item.kind === "command") {
      // Normalize the execution envelope. Covers every command card of any
      // lineage — run_command, plus the legacy match_file / patch×4 /
      // download_file / analyze_locally command cards from before those tools
      // got their own ChatItem kinds. They all render as command cards now:
      // their explain/cmd text carries the history, and the domain fields that
      // were never structured on the old shape simply show empty.
      const cmd = item.cmd as unknown as Record<string, unknown>;
      cmd.execution = ensureExecution(cmd);
      if (!item.result && !item.rejected) {
        item.rejected = { reason: staleCommandReason };
      }
    } else if (item.kind === "patch") {
      const proposal = item.proposal as unknown as Record<string, unknown>;
      proposal.execution = ensureExecution(proposal);
      // A truthy non-string diff would hit proposal.diff.split() in the mv
      // card. Strip rather than crash.
      if (proposal.diff !== undefined && !isStr(proposal.diff)) {
        delete proposal.diff;
      }
      if (!item.result && !item.rejected) {
        item.rejected = { reason: staleCommandReason };
      }
    } else if (item.kind === "match") {
      const proposal = item.proposal as unknown as Record<string, unknown>;
      proposal.execution = ensureExecution(proposal);
      if (!item.result && !item.rejected) {
        item.rejected = { reason: staleCommandReason };
      }
    } else if (item.kind === "web_tool" || item.kind === "download" || item.kind === "analyze") {
      // An unresolved card belongs to a dead actor whose approval ack it will
      // never receive. Mark stale-rejected.
      if (!item.result && !item.rejected) {
        item.rejected = { reason: staleCommandReason };
      }
    }
    items.push(item);
  }
  return items;
}

/** Replay the backend actor's canonical close-time terminal events onto a
 * private timeline snapshot. The actor records these before emitting them, and
 * prepare-stop returns them only after the actor drains. Event callbacks may
 * therefore arrive before or after the invoke reply without changing the
 * persisted result. Every mutation is keyed by message/card/activity id and
 * idempotent. */
export function applyTerminalMutations(
  source: ChatItem[],
  mutations: readonly AiTerminalMutation[],
): ChatItem[] {
  let items = source;
  for (const mutation of mutations) {
    if (!mutation.payload || typeof mutation.payload !== "object") continue;
    const payload = mutation.payload as Record<string, unknown>;
    if (!isStr(payload.id)) continue;

    if (mutation.kind === "assistant_message_end") {
      if (!isStr(payload.text)) continue;
      const index = findLastIndex(items, (item) =>
        item.kind === "assistant" && item.id === payload.id);
      if (index < 0) {
        if (payload.text || payload.cancelled === true) {
          items = [...items, {
            kind: "assistant",
            id: payload.id,
            text: payload.text,
            at: Date.now(),
            streaming: false,
            cancelled: payload.cancelled === true,
          }];
        }
        continue;
      }
      const item = items[index];
      if (item.kind !== "assistant") continue;
      if (!payload.text && payload.cancelled !== true) {
        items = [...items.slice(0, index), ...items.slice(index + 1)];
        continue;
      }
      const replacement: ChatItem = {
        ...item,
        text: payload.text || item.text,
        streaming: false,
        cancelled: payload.cancelled === true,
      };
      items = replaceAt(items, index, replacement);
      continue;
    }

    if (mutation.kind === "command_rejected") {
      if (!isStr(payload.reason)) continue;
      // Reject covers both command and web_tool cards (shared reject channel
      // by id).
      const index = findLastIndex(items, (item) => cardId(item) === payload.id);
      if (index < 0) continue;
      const item = items[index];
      if (item.kind !== "command" && item.kind !== "web_tool" && item.kind !== "download" && item.kind !== "analyze" && item.kind !== "patch" && item.kind !== "match") continue;
      items = replaceAt(items, index, {
        ...item,
        result: undefined,
        rejected: { reason: payload.reason },
      });
      continue;
    }

    if (mutation.kind === "web_tool_completed") {
      if (
        typeof payload.ok !== "boolean"
        || !isStr(payload.summary)
        || typeof payload.duration_ms !== "number"
      ) continue;
      const index = findLastIndex(items, (item) =>
        item.kind === "web_tool" && item.proposal.id === payload.id);
      if (index < 0) continue;
      const item = items[index];
      if (item.kind !== "web_tool") continue;
      items = replaceAt(items, index, {
        ...item,
        rejected: undefined,
        result: {
          id: payload.id,
          ok: payload.ok,
          summary: payload.summary,
          duration_ms: payload.duration_ms,
        },
      });
      continue;
    }

    if (mutation.kind === "download_completed") {
      if (
        typeof payload.ok !== "boolean"
        || !isStr(payload.summary)
        || typeof payload.duration_ms !== "number"
      ) continue;
      const index = findLastIndex(items, (item) =>
        item.kind === "download" && item.proposal.id === payload.id);
      if (index < 0) continue;
      const item = items[index];
      if (item.kind !== "download") continue;
      const result: DownloadResult = {
        id: payload.id,
        ok: payload.ok,
        local_path: isStr(payload.local_path) ? payload.local_path : undefined,
        bytes: typeof payload.bytes === "number" ? payload.bytes : undefined,
        summary: payload.summary,
        duration_ms: payload.duration_ms,
      };
      items = replaceAt(items, index, { ...item, rejected: undefined, result });
      continue;
    }

    if (mutation.kind === "analyze_completed") {
      if (
        typeof payload.ok !== "boolean"
        || !isStr(payload.summary)
        || typeof payload.duration_ms !== "number"
      ) continue;
      const index = findLastIndex(items, (item) =>
        item.kind === "analyze" && item.proposal.id === payload.id);
      if (index < 0) continue;
      const item = items[index];
      if (item.kind !== "analyze") continue;
      items = replaceAt(items, index, {
        ...item,
        rejected: undefined,
        result: {
          id: payload.id,
          ok: payload.ok,
          summary: payload.summary,
          duration_ms: payload.duration_ms,
        },
      });
      continue;
    }

    if (mutation.kind === "patch_completed") {
      // Same PTY-execution result shape as command_completed; matches patch
      // cards (reuses executeCommand, so the registry is shared by id).
      if (
        typeof payload.exit_code !== "number"
        || typeof payload.timed_out !== "boolean"
        || typeof payload.duration_ms !== "number"
        || !isStr(payload.output)
        || typeof payload.original_bytes !== "number"
        || typeof payload.truncated_bytes !== "number"
      ) continue;
      const index = findLastIndex(items, (item) =>
        item.kind === "patch" && item.proposal.id === payload.id);
      if (index < 0) continue;
      const item = items[index];
      if (item.kind !== "patch") continue;
      items = replaceAt(items, index, {
        ...item,
        rejected: undefined,
        result: {
          id: payload.id,
          exit_code: payload.exit_code,
          timed_out: payload.timed_out,
          early_terminated: payload.early_terminated === true,
          duration_ms: payload.duration_ms,
          output: payload.output,
          original_bytes: payload.original_bytes,
          truncated_bytes: payload.truncated_bytes,
        },
      });
      continue;
    }

    if (mutation.kind === "match_completed") {
      // Same PTY-execution result shape; matches match cards.
      if (
        typeof payload.exit_code !== "number"
        || typeof payload.timed_out !== "boolean"
        || typeof payload.duration_ms !== "number"
        || !isStr(payload.output)
        || typeof payload.original_bytes !== "number"
        || typeof payload.truncated_bytes !== "number"
      ) continue;
      const index = findLastIndex(items, (item) =>
        item.kind === "match" && item.proposal.id === payload.id);
      if (index < 0) continue;
      const item = items[index];
      if (item.kind !== "match") continue;
      items = replaceAt(items, index, {
        ...item,
        rejected: undefined,
        result: {
          id: payload.id,
          exit_code: payload.exit_code,
          timed_out: payload.timed_out,
          early_terminated: payload.early_terminated === true,
          duration_ms: payload.duration_ms,
          output: payload.output,
          original_bytes: payload.original_bytes,
          truncated_bytes: payload.truncated_bytes,
        },
      });
      continue;
    }

    if (mutation.kind !== "command_completed") continue;
    if (
      typeof payload.exit_code !== "number"
      || typeof payload.timed_out !== "boolean"
      || typeof payload.duration_ms !== "number"
      || !isStr(payload.output)
      || typeof payload.original_bytes !== "number"
      || typeof payload.truncated_bytes !== "number"
    ) continue;
    const index = findLastIndex(items, (item) =>
      item.kind === "command" && item.cmd.id === payload.id);
    if (index < 0) continue;
    const item = items[index];
    if (item.kind !== "command") continue;
    items = replaceAt(items, index, {
      ...item,
      rejected: undefined,
      result: {
        id: payload.id,
        exit_code: payload.exit_code,
        timed_out: payload.timed_out,
        early_terminated: payload.early_terminated === true,
        duration_ms: payload.duration_ms,
        output: payload.output,
        original_bytes: payload.original_bytes,
        truncated_bytes: payload.truncated_bytes,
      },
    });
  }
  // prepare-stop has drained the actor: no stream can still produce deltas.
  // If the actor panicked before recording its terminal event, persist the
  // partial bubble as cancelled instead of resurrecting a permanent cursor.
  return items.map((item) => {
    if (item.kind === "assistant" && item.streaming) {
      return { ...item, streaming: false, cancelled: true };
    }
    return item;
  });
}

function findLastIndex(
  items: ChatItem[],
  predicate: (item: ChatItem) => boolean,
): number {
  for (let index = items.length - 1; index >= 0; index--) {
    if (predicate(items[index])) return index;
  }
  return -1;
}

function replaceAt(items: ChatItem[], index: number, item: ChatItem): ChatItem[] {
  return [...items.slice(0, index), item, ...items.slice(index + 1)];
}

/** Card id of a proposal-bearing ChatItem (command or web_tool), else null. */
function cardId(item: ChatItem): string | null {
  if (item.kind === "command") return item.cmd.id;
  if (item.kind === "web_tool" || item.kind === "download" || item.kind === "analyze" || item.kind === "patch" || item.kind === "match") return item.proposal.id;
  return null;
}
