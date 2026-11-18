import { invoke } from "@tauri-apps/api/core";
import { createSessionCleanupRegistry, type PendingSessionCleanup } from "../terminal/session-cleanup.ts";

const STATUS_INTERVAL_MS = 5000;
const STATUS_BATCH_SIZE = 256;
let pending = $state<PendingSessionCleanup[]>([]);
let acknowledged = $state<Record<string, string>>({});
let observers = 0;
let statusTimer: ReturnType<typeof setTimeout> | null = null;
let statusPending = false;
let statusGeneration = 0;
const registry = createSessionCleanupRegistry(() => {
  pending = registry.pending();
  const ids = new Set(pending.map(item => item.sessionId));
  for (const id of Object.keys(acknowledged)) if (!ids.has(id)) delete acknowledged[id];
  scheduleStatusCheck();
});

function errorKey(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Each webview owns its cleanup ledger for the lifetime of its session owner. */
export const sessionCleanupScope = registry.forScope;

export function failedSessionCleanups() {
  return pending.flatMap((item) => item.status.kind === "failed"
    ? [{ sessionId: item.sessionId, label: item.label, error: item.status.error }]
    : []);
}

export function sessionCleanupNotices() {
  return failedSessionCleanups().filter(item => acknowledged[item.sessionId] !== errorKey(item.error));
}

export function hiddenSessionCleanupCount(): number {
  return failedSessionCleanups().length - sessionCleanupNotices().length;
}

/** Acknowledging a notice never releases ownership or removes its retry action. */
export function hideSessionCleanup(sessionId: string): void {
  const item = failedSessionCleanups().find(item => item.sessionId === sessionId);
  if (item) acknowledged[sessionId] = errorKey(item.error);
}

export function showSessionCleanupNotices(): void {
  acknowledged = {};
}

function clearStatusTimer(): void {
  if (statusTimer !== null) clearTimeout(statusTimer);
  statusTimer = null;
}

function scheduleStatusCheck(): void {
  if (!observers || failedSessionCleanups().length === 0) {
    clearStatusTimer();
    return;
  }
  if (statusPending || statusTimer !== null) return;
  statusTimer = setTimeout(() => {
    statusTimer = null;
    void checkClosedSessions();
  }, STATUS_INTERVAL_MS);
}

async function checkClosedSessions(): Promise<void> {
  const sessionIds = failedSessionCleanups().map(item => item.sessionId);
  if (!observers || sessionIds.length === 0) return;
  const generation = statusGeneration;
  statusPending = true;
  try {
    for (let offset = 0; offset < sessionIds.length; offset += STATUS_BATCH_SIZE) {
      if (!observers || generation !== statusGeneration) break;
      const batch = sessionIds.slice(offset, offset + STATUS_BATCH_SIZE);
      try {
        // Query the owner's registry only. Reissuing close here would change retry
        // timing and could send additional remote requests while cleanup is pending.
        const closed = await invoke<string[]>("session_cleanup_status", { sessionIds: batch });
        if (observers && generation === statusGeneration) {
          const requested = new Set(batch);
          registry.confirmClosed(closed.filter(id => requested.has(id)));
        }
      } catch (error) {
        // A failed batch must not prevent later sessions from being checked.
        console.warn("[session-cleanup] status check failed:", error);
      }
    }
  } finally {
    statusPending = false;
    scheduleStatusCheck();
  }
}

/** Observe only while the window's notice UI is mounted; no failed entry expires. */
export function observeSessionCleanupStatus(): () => void {
  observers += 1;
  scheduleStatusCheck();
  let stopped = false;
  return () => {
    if (stopped) return;
    stopped = true;
    observers -= 1;
    if (!observers) {
      statusGeneration += 1;
      clearStatusTimer();
    }
  };
}

export async function retrySessionCleanup(sessionId: string): Promise<void> {
  try {
    await registry.retry(sessionId);
  } catch {
    // The ledger retains the new error and the persistent retry control.
  }
}
