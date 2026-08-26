/**
 * Demand-driven feeder in front of terminal.write.
 *
 * Flood output (huge grep/cat) arrives faster than xterm can parse + paint,
 * and xterm's own write buffer is unbounded (it throws past 50 MB). The
 * feeder holds the backlog instead and hands xterm ONE chunk at a time — the
 * next chunk goes out only when the previous one's write callback fired
 * (= it was parsed). The idle path is a direct pass-through with zero added
 * latency, so interactive output (vim/less/keystroke echo) never waits.
 *
 * Unlike the removed write-batcher (#213): no timer, no coalescing. This is
 * accounting plus a release valve, not batching.
 */

interface QueuedChunk {
    data: Uint8Array | string;
    size: number;
}

// Backlog is accounted in bytes. `String.length` counts UTF-16 code units, so
// a CJK string under-reports ~3x and could sneak past the memory cap.
const utf8Encoder = new TextEncoder();

function chunkSize(data: Uint8Array | string): number {
    return typeof data === "string"
        ? utf8Encoder.encode(data).byteLength
        : data.byteLength;
}

export interface OutputFeeder {
    push(data: Uint8Array | string): void;
    /** Bytes not yet parsed by the terminal: queued + the one in flight. */
    pendingBytes(): number;
    /** Discard everything queued. The in-flight chunk (≤1) still parses. */
    dropPending(): void;
    /**
     * Discard incoming pushes until `windowMs` of silence. After a Ctrl+C
     * release, events already queued in the webview keep delivering stale
     * flood bytes; only a quiet gap proves the pipe is drained.
     */
    armQuiescentDrop(windowMs: number): void;
    dispose(): void;
}

export interface OutputFeederOptions {
    write(data: Uint8Array | string, cb: () => void): void;
    maxPendingBytes: number;
    setTimer?: (handler: () => void, ms: number) => ReturnType<typeof setTimeout>;
    clearTimer?: (handle: ReturnType<typeof setTimeout>) => void;
}

export function createOutputFeeder(opts: OutputFeederOptions): OutputFeeder {
    const setTimer = opts.setTimer ?? ((h, ms) => setTimeout(h, ms));
    const clearTimer = opts.clearTimer ?? ((h) => clearTimeout(h));
    const queue: QueuedChunk[] = [];
    let queuedBytes = 0;
    let inflightBytes = 0;              // the single chunk inside write()
    let quiesceMs = 0;
    let quiesceTimer: ReturnType<typeof setTimeout> | null = null;
    let disposed = false;

    function writeChunk(chunk: QueuedChunk) {
        inflightBytes = chunk.size;
        opts.write(chunk.data, () => {
            inflightBytes = 0;
            feedNext();
        });
    }

    function feedNext() {
        if (disposed) return;
        const next = queue.shift();
        if (next === undefined) return;
        queuedBytes -= next.size;
        writeChunk(next);
    }

    return {
        push(data) {
            if (disposed) return;
            if (quiesceTimer !== null) {
                // Stale flood still draining downstream: extend the silence
                // window and drop it.
                clearTimer(quiesceTimer);
                quiesceTimer = setTimer(() => { quiesceTimer = null; }, quiesceMs);
                return;
            }
            const chunk = { data, size: chunkSize(data) };
            // Memory cap: drop OLDEST whole chunks. Flood output is garbage
            // the user is about to interrupt anyway; the seam may mis-render
            // one line. Acceptable.
            while (queue.length > 0
                && queuedBytes + inflightBytes + chunk.size > opts.maxPendingBytes) {
                const oldest = queue.shift()!;
                queuedBytes -= oldest.size;
            }
            if (queue.length === 0 && inflightBytes === 0) {
                writeChunk(chunk);      // idle: straight through, no timer
                return;
            }
            queue.push(chunk);
            queuedBytes += chunk.size;
        },
        pendingBytes() {
            return queuedBytes + inflightBytes;
        },
        dropPending() {
            queue.length = 0;
            queuedBytes = 0;
        },
        armQuiescentDrop(windowMs) {
            if (disposed) return;
            quiesceMs = windowMs;
            if (quiesceTimer !== null) clearTimer(quiesceTimer);
            quiesceTimer = setTimer(() => { quiesceTimer = null; }, windowMs);
        },
        dispose() {
            disposed = true;
            queue.length = 0;
            queuedBytes = 0;
            if (quiesceTimer !== null) {
                clearTimer(quiesceTimer);
                quiesceTimer = null;
            }
        },
    };
}

/** 500 B / 2 KB / 1.4 MB — for the backlog badge. */
export function formatBacklogBytes(n: number): string {
    if (n >= 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
    if (n >= 1024) return `${Math.round(n / 1024)} KB`;
    return `${n} B`;
}
