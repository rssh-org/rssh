type CloseSession = (sessionId: string) => void | Promise<void>;

export interface PendingSessionCleanup {
  sessionId: string;
  scope: string;
  label: string;
  status: { kind: "closing" } | { kind: "failed"; error: unknown };
}

export interface SessionCleanupScope {
  requestClose(sessionId: string, close: CloseSession): void;
  wait(shouldContinue?: () => boolean): Promise<void>;
}

/** Owned by the window, not a terminal pane: a failed close still owns a handle. */
export function createSessionCleanupRegistry(changed: () => void = () => {}) {
  type Entry = PendingSessionCleanup & {
    close: CloseSession;
    inFlight?: Promise<void>;
  };
  const entries = new Map<string, Entry>();

  function run(entry: Entry): Promise<void> {
    if (entry.inFlight) return entry.inFlight;
    let resolve!: () => void;
    let reject!: (error: unknown) => void;
    const task = new Promise<void>((done, failed) => { resolve = done; reject = failed; });
    entry.inFlight = task;
    entry.status = { kind: "closing" };
    changed();
    const succeeded = () => {
      entries.delete(entry.sessionId);
      changed();
      resolve();
    };
    const failed = (error: unknown) => {
      entry.inFlight = undefined;
      entry.status = { kind: "failed", error };
      changed();
      reject(error);
    };
    try {
      Promise.resolve(entry.close(entry.sessionId)).then(succeeded, failed);
    } catch (error) {
      failed(error);
    }
    return task;
  }

  return {
    pending(): PendingSessionCleanup[] {
      return Array.from(entries.values(), ({ sessionId, scope, label, status }) => ({ sessionId, scope, label, status }));
    },
    async retry(sessionId: string): Promise<void> {
      const entry = entries.get(sessionId);
      if (entry) await run(entry);
    },
    forScope(scope: string, label: string): SessionCleanupScope {
      return {
        requestClose(sessionId, close) {
          let entry = entries.get(sessionId);
          if (!entry) {
            entry = { sessionId, scope, label, status: { kind: "closing" }, close };
            entries.set(sessionId, entry);
          }
          // Fire-and-forget cancellation is safe because failure remains owned
          // and visible here; it is never converted into a successful close.
          void run(entry).catch(() => {});
        },
        async wait(shouldContinue = () => true) {
          // A late open result can add another handle while a close is pending.
          // Each failed close stops this reconnect; only a new user action retries.
          while (shouldContinue()) {
            const pending = Array.from(entries.values()).filter((entry) => entry.scope === scope);
            if (pending.length === 0) return;
            await Promise.all(pending.map(run));
          }
        },
      };
    },
  };
}
