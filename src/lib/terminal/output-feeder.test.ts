import { describe, expect, it } from "vitest";
import { Terminal } from "@xterm/xterm";

import {
    createOutputFeeder,
    formatBacklogBytes,
} from "./output-feeder.ts";

/** Capturing write stub: records data, hands back the drain trigger. */
function stubWrite() {
    const written: (Uint8Array | string)[] = [];
    let cb: (() => void) | null = null;
    return {
        write: (data: Uint8Array | string, onDone: () => void) => {
            written.push(data);
            cb = onDone;
        },
        written,
        drain: () => { const c = cb; cb = null; c?.(); },
    };
}

/** Injectable fake timers (same pattern the removed write-batcher tests used). */
function fakeTimers() {
    let seq = 0;
    const pending = new Map<number, () => void>();
    return {
        setTimer: (h: () => void, _ms: number) => {
            const id = ++seq;
            pending.set(id, h);
            return id as unknown as ReturnType<typeof setTimeout>;
        },
        clearTimer: (id: ReturnType<typeof setTimeout>) => {
            pending.delete(id as unknown as number);
        },
        fire: () => {
            const first = pending.entries().next().value;
            if (first) { pending.delete(first[0]); first[1](); }
        },
        count: () => pending.size,
    };
}

describe("createOutputFeeder", () => {
    it("idle push passes straight through — write called synchronously", () => {
        const w = stubWrite();
        const f = createOutputFeeder({ write: w.write, maxPendingBytes: 1024 });
        f.push("a");
        expect(w.written).toEqual(["a"]);   // no timer, no queue
        expect(f.pendingBytes()).toBe(1);   // in flight until parsed
        w.drain();
        expect(f.pendingBytes()).toBe(0);
    });

    it("push while busy queues; drains exactly one chunk per callback", () => {
        const w = stubWrite();
        const f = createOutputFeeder({ write: w.write, maxPendingBytes: 1024 });
        f.push("aaa");
        f.push("bb");
        f.push("c");
        expect(w.written).toEqual(["aaa"]);
        expect(f.pendingBytes()).toBe(6);
        w.drain();
        expect(w.written).toEqual(["aaa", "bb"]);
        expect(f.pendingBytes()).toBe(3);
        w.drain();
        expect(w.written).toEqual(["aaa", "bb", "c"]);
        w.drain();
        expect(f.pendingBytes()).toBe(0);
    });

    it("dropPending clears the queue; the in-flight chunk finishes cleanly", () => {
        const w = stubWrite();
        const f = createOutputFeeder({ write: w.write, maxPendingBytes: 1024 });
        f.push("aa");      // in flight
        f.push("bb");      // queued
        f.push("cc");      // queued
        f.dropPending();
        expect(f.pendingBytes()).toBe(2);   // only the in-flight "aa"
        w.drain();
        expect(w.written).toEqual(["aa"]);  // queued data never written
    });

    it("quiescent drop discards pushes until the gap timer fires", () => {
        const w = stubWrite();
        const t = fakeTimers();
        const f = createOutputFeeder({
            write: w.write, maxPendingBytes: 1024,
            setTimer: t.setTimer, clearTimer: t.clearTimer,
        });
        f.armQuiescentDrop(150);
        f.push("stale");
        expect(w.written).toEqual([]);
        t.fire();          // silence window elapsed
        f.push("fresh");
        expect(w.written).toEqual(["fresh"]);
    });

    it("each stale push re-arms the gap timer", () => {
        const w = stubWrite();
        const t = fakeTimers();
        const f = createOutputFeeder({
            write: w.write, maxPendingBytes: 1024,
            setTimer: t.setTimer, clearTimer: t.clearTimer,
        });
        f.armQuiescentDrop(150);
        f.push("stale1");
        expect(t.count()).toBe(1);
        f.push("stale2");  // arrival within the window resets it
        expect(t.count()).toBe(1);   // re-armed, old timer cleared
        expect(w.written).toEqual([]);
        t.fire();          // silence window finally elapses
        f.push("fresh");
        expect(w.written).toEqual(["fresh"]);
    });

    it("over-cap drops the OLDEST whole chunks first", () => {
        const w = stubWrite();
        const f = createOutputFeeder({ write: w.write, maxPendingBytes: 5 });
        f.push("aa");      // in flight (2 B)
        f.push("bbb");     // queued (3 B)
        f.push("cc");      // 2+3+2 > 5 → "bbb" dropped
        expect(f.pendingBytes()).toBe(4);
        w.drain();
        expect(w.written).toEqual(["aa", "cc"]);
    });

    it("dispose stops feeding; idempotent", () => {
        const w = stubWrite();
        const f = createOutputFeeder({ write: w.write, maxPendingBytes: 1024 });
        f.push("aa");
        f.push("bb");
        f.dispose();
        w.drain();         // in-flight callback fires after dispose
        expect(w.written).toEqual(["aa"]);  // "bb" never fed
        f.dispose();       // no throw
    });

    it("string chunks are accounted in UTF-8 bytes, not UTF-16 code units", () => {
        const w = stubWrite();
        const f = createOutputFeeder({ write: w.write, maxPendingBytes: 1024 });
        f.push("汉");          // 1 code unit, 3 UTF-8 bytes
        expect(f.pendingBytes()).toBe(3);
        f.push("aé");          // 2 code units, 3 UTF-8 bytes
        expect(f.pendingBytes()).toBe(6);
    });

    it("integration: feeds a real Terminal and pending settles to 0", async () => {
        const term = new Terminal({ allowProposedApi: true, scrollback: 1000 });
        const f = createOutputFeeder({
            write: (data, cb) => term.write(data, cb),
            maxPendingBytes: 1024,
        });
        f.push("hello\r\nworld\r\n");
        f.push("third line\r\n");
        // Poll until the feeder reports drained (bounded), instead of a fixed
        // sleep that flakes on loaded CI.
        const deadline = Date.now() + 5000;
        while (f.pendingBytes() > 0 && Date.now() < deadline) {
            await new Promise((r) => setTimeout(r, 10));
        }
        expect(f.pendingBytes()).toBe(0);
        expect(term.buffer.active.getLine(2)!.translateToString(true)).toBe("third line");
        term.dispose();
    });

    it("formatBacklogBytes renders human units", () => {
        expect(formatBacklogBytes(500)).toBe("500 B");
        expect(formatBacklogBytes(2048)).toBe("2 KB");
        expect(formatBacklogBytes(1_500_000)).toBe("1.4 MB");
    });
});
