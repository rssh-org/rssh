import { createSessionCleanupRegistry, type PendingSessionCleanup } from "../terminal/session-cleanup.ts";

let pending = $state<PendingSessionCleanup[]>([]);
const registry = createSessionCleanupRegistry(() => { pending = registry.pending(); });

/** Each webview owns its cleanup ledger for the lifetime of its session owner. */
export const sessionCleanupScope = registry.forScope;

export function failedSessionCleanups() {
  return pending.flatMap((item) => item.status.kind === "failed"
    ? [{ sessionId: item.sessionId, label: item.label, error: item.status.error }]
    : []);
}

export async function retrySessionCleanup(sessionId: string): Promise<void> {
  try {
    await registry.retry(sessionId);
  } catch {
    // The ledger retains the new error and the persistent retry control.
  }
}
