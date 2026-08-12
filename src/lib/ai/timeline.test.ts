import { describe, it, expect } from "vitest";
import { applyTerminalMutations, restoreTimeline } from "./timeline.ts";
import type { ChatItem } from "./types.ts";

const STALE = "stale";

function roundtrip(items: unknown[]): ChatItem[] {
  return restoreTimeline(JSON.stringify(items), STALE);
}

describe("restoreTimeline", () => {
  it("returns [] for corrupt or non-array json", () => {
    expect(restoreTimeline("not json", STALE)).toEqual([]);
    expect(restoreTimeline('{"kind":"user"}', STALE)).toEqual([]);
  });

  it("preserves plain bubbles verbatim", () => {
    const items = [
      { kind: "user", text: "hi", at: 1 },
      { kind: "assistant", id: "a1", text: "hello", at: 2, streaming: false },
      { kind: "error", text: "boom", at: 3 },
      { kind: "note", text: "n", at: 4 },
    ];
    expect(roundtrip(items)).toEqual(items);
  });

  it("drops live-only user mutation metadata from legacy timelines", () => {
    expect(roundtrip([
      {
        kind: "user",
        client_id: "old-instance:100",
        client_seq: 100,
        text: "from an earlier runtime",
        at: 1,
      },
    ])).toEqual([
      { kind: "user", text: "from an earlier runtime", at: 1 },
    ]);
  });

  it("kills a resurrected streaming cursor", () => {
    const [a] = roundtrip([
      { kind: "assistant", id: "a1", text: "partial", at: 1, streaming: true },
    ]);
    expect(a.kind === "assistant" && a.streaming).toBe(false);
  });

  it("drops an empty non-cancelled assistant placeholder, keeps a cancelled one", () => {
    const items = roundtrip([
      { kind: "assistant", id: "a1", text: "", at: 1, streaming: true },
      { kind: "assistant", id: "a2", text: "", at: 2, streaming: false, cancelled: true },
    ]);
    expect(items).toHaveLength(1);
    expect(items[0].kind === "assistant" && items[0].id).toBe("a2");
  });

  it("marks an unresolved command card as stale-rejected", () => {
    const [c] = roundtrip([
      { kind: "command", cmd: { id: "c1", cmd: "ls" }, at: 1 },
    ]);
    expect(c.kind === "command" && c.rejected?.reason).toBe(STALE);
  });

  it("leaves resolved and rejected command cards alone", () => {
    const items = roundtrip([
      { kind: "command", cmd: { id: "c1", cmd: "ls" }, at: 1, result: { id: "c1", exit_code: 0 } },
      { kind: "command", cmd: { id: "c2", cmd: "ls" }, at: 2, rejected: { reason: "user said no" } },
    ]);
    expect(items[0].kind === "command" && items[0].rejected).toBeUndefined();
    expect(items[1].kind === "command" && items[1].rejected?.reason).toBe("user said no");
  });

  it("drops unknown kinds and garbage entries", () => {
    const items = roundtrip([
      null,
      42,
      { kind: "alien", at: 1 },
      { kind: "user", text: "kept", at: 2 },
    ]);
    expect(items).toEqual([{ kind: "user", text: "kept", at: 2 }]);
  });

  it("strips a non-string diff instead of crashing the diff renderer", () => {
    const [c] = roundtrip([
      { kind: "command", cmd: { id: "c1", cmd: "ls", diff: 42 }, at: 1, rejected: { reason: "no" } },
    ]);
    expect(c.kind === "command" && c.cmd.diff).toBeUndefined();
  });

  it("drops known kinds with mangled bodies instead of crashing render", () => {
    const items = roundtrip([
      { kind: "command", at: 1 },                       // no cmd object
      { kind: "command", cmd: { id: 7, cmd: "x" } },    // id not a string
      { kind: "user", at: 2 },                          // no text
      { kind: "assistant", text: "no id", at: 3 },      // no id
      { kind: "note", text: "no timestamp" },           // no at → "Invalid Date"
      { kind: "note", text: "ok", at: 4 },
    ]);
    expect(items).toEqual([{ kind: "note", text: "ok", at: 4 }]);
  });

  it("marks an unresolved web_tool card as stale-rejected", () => {
    const [w] = roundtrip([
      { kind: "web_tool", proposal: { id: "w1", kind: "web_fetch", target: "https://x" }, at: 1 },
    ]);
    expect(w.kind === "web_tool" && w.rejected?.reason).toBe(STALE);
  });

  it("leaves resolved and rejected web_tool cards alone", () => {
    const items = roundtrip([
      {
        kind: "web_tool", proposal: { id: "w1", kind: "web_search", target: "q" }, at: 1,
        result: { id: "w1", ok: true, summary: "s", duration_ms: 1 },
      },
      {
        kind: "web_tool", proposal: { id: "w2", kind: "web_fetch", target: "https://x" }, at: 2,
        rejected: { reason: "user said no" },
      },
    ]);
    expect(items[0].kind === "web_tool" && items[0].rejected).toBeUndefined();
    expect(items[1].kind === "web_tool" && items[1].rejected?.reason).toBe("user said no");
  });

  it("drops web_tool cards with a mangled proposal shape", () => {
    const items = roundtrip([
      { kind: "web_tool", proposal: { id: "w1", kind: "bogus", target: "x" }, at: 1 }, // bad kind
      { kind: "web_tool", proposal: { id: "w2", kind: "web_fetch" }, at: 2 },          // missing target
      { kind: "web_tool", at: 3 },                                                      // missing proposal
      { kind: "user", text: "kept", at: 4 },
    ]);
    expect(items).toEqual([{ kind: "user", text: "kept", at: 4 }]);
  });

  // --- download (independent line, ack-only) ---
  it("marks an unresolved download card as stale-rejected", () => {
    const [d] = roundtrip([
      { kind: "download", proposal: { id: "d1", remote_path: "/var/log/x", max_mb: 50, dest_dir: "/tmp/x" }, at: 1 },
    ]);
    expect(d.kind === "download" && d.rejected?.reason).toBe(STALE);
  });

  it("drops legacy command/download_file cards from before the split", () => {
    // download_file migrated to its own ChatItem.download — a stale blob may
    // still hold it as a command card. Drop it, don't resurrect an orphan
    // whose full_cmd/sentinel are empty under the new model.
    const items = roundtrip([
      { kind: "command", cmd: { id: "d1", tool_call_id: "d1", cmd: "download_file: /x", full_cmd: "", sentinel: "", explain: "", side_effect: "", timeout_s: 600, kind: "download_file" }, at: 1 },
      { kind: "user", text: "kept", at: 2 },
    ]);
    expect(items).toEqual([{ kind: "user", text: "kept", at: 2 }]);
  });
});

describe("applyTerminalMutations", () => {
  it("idempotently closes a streaming assistant bubble with canonical text", () => {
    const source: ChatItem[] = [{
      kind: "assistant",
      id: "reply-1",
      text: "partial",
      at: 1,
      streaming: true,
    }];
    const terminal = [{
      kind: "assistant_message_end",
      payload: { id: "reply-1", text: "partial response", cancelled: true },
    }];

    const once = applyTerminalMutations(source, terminal);
    const twice = applyTerminalMutations(once, terminal);

    expect(twice).toEqual([{
      kind: "assistant",
      id: "reply-1",
      text: "partial response",
      at: 1,
      streaming: false,
      cancelled: true,
    }]);
  });

  it("replays backend-sanitized command completion and rejection by card id", () => {
    const command = (id: string): ChatItem => ({
      kind: "command",
      cmd: {
        id,
        tool_call_id: `tool-${id}`,
        cmd: "show secret",
        full_cmd: "show secret",
        sentinel: "sentinel",
        explain: "",
        side_effect: "",
        timeout_s: 30,
      },
      at: 1,
    });

    expect(applyTerminalMutations(
      [command("complete"), command("reject")],
      [
        {
          kind: "command_completed",
          payload: {
            id: "complete",
            exit_code: 0,
            timed_out: false,
            early_terminated: true,
            duration_ms: 12,
            output: "[REDACTED]",
            original_bytes: 100,
            truncated_bytes: 80,
            lock_keyboard: false,
          },
        },
        {
          kind: "command_rejected",
          payload: { id: "reject", reason: "not now" },
        },
      ],
    )).toEqual([
      expect.objectContaining({
        kind: "command",
        result: {
          id: "complete",
          exit_code: 0,
          timed_out: false,
          early_terminated: true,
          duration_ms: 12,
          output: "[REDACTED]",
          original_bytes: 100,
          truncated_bytes: 80,
        },
      }),
      expect.objectContaining({
        kind: "command",
        rejected: { reason: "not now" },
      }),
    ]);
  });

  it("drops an empty failed assistant placeholder and ignores malformed mutations", () => {
    const source: ChatItem[] = [{
      kind: "assistant",
      id: "reply-1",
      text: "",
      at: 1,
      streaming: true,
    }];

    expect(applyTerminalMutations(source, [
      { kind: "command_completed", payload: { id: "missing" } },
      { kind: "assistant_message_end", payload: { id: "reply-1", text: "" } },
    ])).toEqual([]);
  });

  it("reconstructs a missing non-empty assistant end but not an empty failed one", () => {
    expect(applyTerminalMutations([], [
      {
        kind: "assistant_message_end",
        payload: { id: "reply-visible", text: "final response" },
      },
      {
        kind: "assistant_message_end",
        payload: { id: "reply-empty", text: "" },
      },
    ])).toEqual([{
      kind: "assistant",
      id: "reply-visible",
      text: "final response",
      at: expect.any(Number),
      streaming: false,
      cancelled: false,
    }]);
  });

  it("settles a leftover stream if the actor exits without a terminal mutation", () => {
    expect(applyTerminalMutations([{
      kind: "assistant",
      id: "reply-panic",
      text: "partial",
      at: 1,
      streaming: true,
    }], [])).toEqual([{
      kind: "assistant",
      id: "reply-panic",
      text: "partial",
      at: 1,
      streaming: false,
      cancelled: true,
    }]);
  });

  it("replays web_tool completion (ok and failed) by proposal id", () => {
    const card = (id: string): ChatItem => ({
      kind: "web_tool",
      proposal: { id, kind: "web_fetch", target: "https://example.com" },
      at: 1,
    });
    const mutations = [
      { kind: "web_tool_completed", payload: { id: "ok", ok: true, summary: "done", duration_ms: 5 } },
      { kind: "web_tool_completed", payload: { id: "fail", ok: false, summary: "boom", duration_ms: 7 } },
    ];
    const once = applyTerminalMutations([card("ok"), card("fail")], mutations);
    // Idempotent: replaying the same terminal events must not double-apply.
    const twice = applyTerminalMutations(once, mutations);
    expect(twice).toEqual([
      expect.objectContaining({
        kind: "web_tool",
        proposal: expect.objectContaining({ id: "ok" }),
        result: { id: "ok", ok: true, summary: "done", duration_ms: 5 },
      }),
      expect.objectContaining({
        kind: "web_tool",
        proposal: expect.objectContaining({ id: "fail" }),
        result: { id: "fail", ok: false, summary: "boom", duration_ms: 7 },
      }),
    ]);
  });

  it("drops malformed web_tool_completed payloads without touching the card", () => {
    const card: ChatItem = {
      kind: "web_tool",
      proposal: { id: "w1", kind: "web_search", target: "query" },
      at: 1,
    };
    expect(applyTerminalMutations([card], [
      { kind: "web_tool_completed", payload: { id: "w1", summary: "s", duration_ms: 1 } }, // missing ok
      { kind: "web_tool_completed", payload: { id: "w1", ok: true, duration_ms: 1 } },     // missing summary
      { kind: "web_tool_completed", payload: { id: "w1", ok: true, summary: "s" } },       // missing duration_ms
      { kind: "web_tool_completed", payload: { id: "missing", ok: true, summary: "s", duration_ms: 1 } }, // no card
    ])).toEqual([card]);
  });

  it("rejects a web_tool card through the shared command_rejected channel", () => {
    const card: ChatItem = {
      kind: "web_tool",
      proposal: { id: "w1", kind: "web_fetch", target: "https://x" },
      at: 1,
      result: { id: "w1", ok: true, summary: "stale", duration_ms: 1 },
    };
    expect(applyTerminalMutations([card], [
      { kind: "command_rejected", payload: { id: "w1", reason: "nope" } },
    ])).toEqual([
      expect.objectContaining({
        kind: "web_tool",
        rejected: { reason: "nope" },
        result: undefined,
      }),
    ]);
  });

  it("replays download completion (ok with local_path, failed without) by proposal id", () => {
    const card = (id: string): ChatItem => ({
      kind: "download",
      proposal: { id, remote_path: "/var/log/x", max_mb: 50, dest_dir: "/tmp/x" },
      at: 1,
    });
    const mutations = [
      { kind: "download_completed", payload: { id: "ok", ok: true, local_path: "/tmp/x/log", bytes: 1024, summary: "Downloaded 1024 bytes", duration_ms: 5 } },
      { kind: "download_completed", payload: { id: "fail", ok: false, summary: "Download failed", duration_ms: 7 } },
    ];
    const once = applyTerminalMutations([card("ok"), card("fail")], mutations);
    // Idempotent: replaying the same terminal events must not double-apply.
    const twice = applyTerminalMutations(once, mutations);
    expect(twice).toEqual([
      expect.objectContaining({
        kind: "download",
        proposal: expect.objectContaining({ id: "ok" }),
        result: { id: "ok", ok: true, local_path: "/tmp/x/log", bytes: 1024, summary: "Downloaded 1024 bytes", duration_ms: 5 },
      }),
      expect.objectContaining({
        kind: "download",
        proposal: expect.objectContaining({ id: "fail" }),
        result: { id: "fail", ok: false, local_path: undefined, bytes: undefined, summary: "Download failed", duration_ms: 7 },
      }),
    ]);
  });

  it("rejects a download card through the shared command_rejected channel", () => {
    const card: ChatItem = {
      kind: "download",
      proposal: { id: "d1", remote_path: "/x", max_mb: 50, dest_dir: "/tmp" },
      at: 1,
      result: { id: "d1", ok: true, summary: "stale", duration_ms: 1 },
    };
    expect(applyTerminalMutations([card], [
      { kind: "command_rejected", payload: { id: "d1", reason: "nope" } },
    ])).toEqual([
      expect.objectContaining({
        kind: "download",
        rejected: { reason: "nope" },
        result: undefined,
      }),
    ]);
  });

});
