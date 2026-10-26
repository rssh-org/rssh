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

  it("migrates a pre-split command card's flat execution into the envelope", () => {
    // Pre-PtyExecution-split blobs carried full_cmd/sentinel/timeout_s flat.
    // CommandConfirmDialog now reads cmd.execution.timeout_s — normalize so an
    // old conversation opens instead of crashing the render.
    const [c] = roundtrip([
      {
        kind: "command",
        cmd: {
          id: "c1", tool_call_id: "c1", cmd: "df -h",
          full_cmd: "df -h; echo s:$?", sentinel: "s", timeout_s: 30,
          explain: "check disk", side_effect: "read-only", kind: "run_command",
        },
        at: 1, result: { id: "c1", exit_code: 0 },
      },
    ]);
    if (!(c.kind === "command")) throw new Error("expected command");
    expect(c.cmd.execution).toEqual({ full_cmd: "df -h; echo s:$?", sentinel: "s", timeout_s: 30 });
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

  it("drops entries that aren't a card (no payload / no text / no timestamp)", () => {
    const items = roundtrip([
      { kind: "command", at: 1 },                       // no cmd payload
      { kind: "user", at: 2 },                          // no text
      { kind: "assistant", text: "no id", at: 3 },      // no id
      { kind: "note", text: "no timestamp" },           // no at → "Invalid Date"
      { kind: "note", text: "ok", at: 4 },
    ]);
    expect(items).toEqual([{ kind: "note", text: "ok", at: 4 }]);
  });

  it("renders a command card with weak fields instead of dropping it", () => {
    // A command card is a card as long as it has a cmd object — missing/typed
    // fields no longer drop it, they render empty (execution normalized to
    // defaults). No more per-field validation that has to mirror the template.
    const [c] = roundtrip([
      { kind: "command", cmd: { id: 7, cmd: "x" }, at: 1, rejected: { reason: "no" } },
    ]);
    if (!(c.kind === "command")) throw new Error("expected command");
    expect(c.cmd.execution).toEqual({ full_cmd: "", sentinel: "", timeout_s: 0 });
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

  it("keeps web_tool cards with partial proposals, drops only those with no proposal", () => {
    const items = roundtrip([
      { kind: "web_tool", proposal: { id: "w1", kind: "bogus", target: "x" }, at: 1 },
      { kind: "web_tool", proposal: { id: "w2", kind: "web_fetch" }, at: 2 },
      { kind: "web_tool", at: 3 }, // no proposal → not a card
      { kind: "user", text: "kept", at: 4 },
    ]);
    expect(items).toHaveLength(3);
    expect(items[0].kind === "web_tool" && items[0].proposal.id).toBe("w1");
    expect(items[1].kind === "web_tool" && items[1].proposal.id).toBe("w2");
  });

  // --- download (independent line, ack-only) ---
  it("marks an unresolved download card as stale-rejected", () => {
    const [d] = roundtrip([
      { kind: "download", proposal: { id: "d1", remote_path: "/var/log/x", max_mb: 50, dest_dir: "/tmp/x" }, at: 1 },
    ]);
    expect(d.kind === "download" && d.rejected?.reason).toBe(STALE);
  });

  it("renders legacy download_file command cards as command cards", () => {
    // download_file used to ride the command line; an old blob may still hold
    // it that way. It renders as a command card (execution normalized) rather
    // than disappearing — explain/cmd text carries the history.
    const [c, u] = roundtrip([
      { kind: "command", cmd: { id: "d1", tool_call_id: "d1", cmd: "download_file: /x", full_cmd: "", sentinel: "", explain: "download /x", side_effect: "", timeout_s: 600, kind: "download_file" }, at: 1, rejected: { reason: "no" } },
      { kind: "user", text: "kept", at: 2 },
    ]);
    if (!(c.kind === "command")) throw new Error("expected command");
    expect(c.cmd.kind).toBe("download_file");
    expect(c.cmd.execution).toEqual({ full_cmd: "", sentinel: "", timeout_s: 600 });
    expect(u.kind).toBe("user");
  });

  // --- patch (independent line, PTY execution via executeCommand) ---
  it("marks an unresolved patch card as stale-rejected", () => {
    const [p] = roundtrip([
      { kind: "patch", proposal: { id: "p1", step: "cp", path: "/etc/x", tmp_path: "/etc/x.rssh-1", cmd: "cp ..." }, at: 1 },
    ]);
    expect(p.kind === "patch" && p.rejected?.reason).toBe(STALE);
  });

  it("strips a non-string patch diff instead of crashing the diff renderer", () => {
    const [p] = roundtrip([
      { kind: "patch", proposal: { id: "p1", step: "mv", path: "/etc/x", diff: 42, cmd: "mv ..." }, at: 1, rejected: { reason: "no" } },
    ]);
    expect(p.kind === "patch" && p.proposal.diff).toBeUndefined();
  });

  it("renders legacy patch_* command cards as command cards", () => {
    // patch×4 used to ride the command line; old blobs hold them as command
    // cards. They render as command cards (execution normalized) rather than
    // disappearing.
    const items = roundtrip([
      { kind: "command", cmd: { id: "p1", tool_call_id: "p1", cmd: "cp", full_cmd: "", sentinel: "", explain: "patch 1/4", side_effect: "", timeout_s: 30, kind: "patch_cp" }, at: 1, rejected: { reason: "no" } },
      { kind: "command", cmd: { id: "p2", tool_call_id: "p2", cmd: "mv", full_cmd: "", sentinel: "", explain: "patch 4/4", side_effect: "", timeout_s: 30, kind: "patch_mv" }, at: 2, rejected: { reason: "no" } },
      { kind: "user", text: "kept", at: 3 },
    ]);
    expect(items).toHaveLength(3);
    if (!(items[0].kind === "command")) throw new Error("expected command");
    expect(items[0].cmd.kind).toBe("patch_cp");
    expect(items[1].kind === "command" && items[1].cmd.kind).toBe("patch_mv");
  });

  it("keeps patch cards with partial proposals, drops only those with no proposal", () => {
    const items = roundtrip([
      { kind: "patch", proposal: { id: "p1", step: "bogus", path: "/x" }, at: 1, rejected: { reason: "no" } },
      { kind: "patch", proposal: { id: "p2", step: "cp" }, at: 2, rejected: { reason: "no" } },
      { kind: "patch", at: 3 }, // no proposal → not a card
      { kind: "user", text: "kept", at: 4 },
    ]);
    expect(items).toHaveLength(3);
    expect(items[0].kind === "patch" && items[0].proposal.id).toBe("p1");
    expect(items[1].kind === "patch" && items[1].proposal.id).toBe("p2");
  });

  // --- match (independent line, read-only PTY search) ---
  it("marks an unresolved match card as stale-rejected", () => {
    const [m] = roundtrip([
      { kind: "match", proposal: { id: "m1", path: "/etc/x", find: "foo", before: 80, after: 80, cmd: "grep ..." }, at: 1 },
    ]);
    expect(m.kind === "match" && m.rejected?.reason).toBe(STALE);
  });

  it("renders legacy match_file command cards as command cards", () => {
    const [c, u] = roundtrip([
      { kind: "command", cmd: { id: "m1", tool_call_id: "m1", cmd: "match", full_cmd: "", sentinel: "", explain: "match_file: search /x", side_effect: "", timeout_s: 60, kind: "match_file" }, at: 1, rejected: { reason: "no" } },
      { kind: "user", text: "kept", at: 2 },
    ]);
    if (!(c.kind === "command")) throw new Error("expected command");
    expect(c.cmd.kind).toBe("match_file");
    expect(c.cmd.execution).toEqual({ full_cmd: "", sentinel: "", timeout_s: 60 });
    expect(u.kind).toBe("user");
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
        explain: "",
        side_effect: "",
        execution: { full_cmd: "show secret", sentinel: "sentinel", timeout_s: 30 },
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

  it("replays patch completion (pty result) by proposal id", () => {
    const card = (id: string): ChatItem => ({
      kind: "patch",
      proposal: { id, step: "cp", path: "/etc/x", tmp_path: "/etc/x.rssh-1", cmd: "cp ...", execution: { full_cmd: "cp ...", sentinel: "s", timeout_s: 30 } },
      at: 1,
    });
    const mutations = [
      { kind: "patch_completed", payload: { id: "ok", exit_code: 0, timed_out: false, duration_ms: 12, output: "", original_bytes: 0, truncated_bytes: 0 } },
      { kind: "patch_completed", payload: { id: "fail", exit_code: 1, timed_out: false, duration_ms: 7, output: "boom", original_bytes: 4, truncated_bytes: 0 } },
    ];
    const once = applyTerminalMutations([card("ok"), card("fail")], mutations);
    // Idempotent: replaying the same terminal events must not double-apply.
    const twice = applyTerminalMutations(once, mutations);
    expect(twice).toEqual([
      expect.objectContaining({
        kind: "patch",
        result: { id: "ok", exit_code: 0, timed_out: false, early_terminated: false, duration_ms: 12, output: "", original_bytes: 0, truncated_bytes: 0 },
      }),
      expect.objectContaining({
        kind: "patch",
        result: { id: "fail", exit_code: 1, timed_out: false, early_terminated: false, duration_ms: 7, output: "boom", original_bytes: 4, truncated_bytes: 0 },
      }),
    ]);
  });

  it("drops malformed patch_completed payloads without touching the card", () => {
    const card: ChatItem = {
      kind: "patch",
      proposal: { id: "p1", step: "cp", path: "/x", cmd: "cp ...", execution: { full_cmd: "cp ...", sentinel: "s", timeout_s: 30 } },
      at: 1,
    };
    expect(applyTerminalMutations([card], [
      { kind: "patch_completed", payload: { id: "p1", exit_code: 0, timed_out: false, duration_ms: 1, output: "", original_bytes: 0 } }, // missing truncated_bytes
      { kind: "patch_completed", payload: { id: "missing", exit_code: 0, timed_out: false, duration_ms: 1, output: "", original_bytes: 0, truncated_bytes: 0 } }, // no card
    ])).toEqual([card]);
  });

  it("rejects a patch card through the shared command_rejected channel", () => {
    const card: ChatItem = {
      kind: "patch",
      proposal: { id: "p1", step: "mv", path: "/etc/x", diff: "--- x\n+++ x\n", cmd: "mv ...", execution: { full_cmd: "mv ...", sentinel: "s", timeout_s: 30 } },
      at: 1,
      result: { id: "p1", exit_code: 0, timed_out: false, duration_ms: 1, output: "", original_bytes: 0, truncated_bytes: 0 },
    };
    expect(applyTerminalMutations([card], [
      { kind: "command_rejected", payload: { id: "p1", reason: "nope" } },
    ])).toEqual([
      expect.objectContaining({
        kind: "patch",
        rejected: { reason: "nope" },
        result: undefined,
      }),
    ]);
  });

  it("replays match completion (pty result) by proposal id", () => {
    const card: ChatItem = {
      kind: "match",
      proposal: { id: "m1", path: "/etc/x", find: "foo", before: 80, after: 80, cmd: "grep ...", execution: { full_cmd: "grep ...", sentinel: "s", timeout_s: 60 } },
      at: 1,
    };
    const mutations = [
      { kind: "match_completed", payload: { id: "m1", exit_code: 0, timed_out: false, duration_ms: 9, output: '{"count":1}', original_bytes: 12, truncated_bytes: 0 } },
    ];
    const once = applyTerminalMutations([card], mutations);
    const twice = applyTerminalMutations(once, mutations);
    expect(twice).toEqual([
      expect.objectContaining({
        kind: "match",
        result: { id: "m1", exit_code: 0, timed_out: false, early_terminated: false, duration_ms: 9, output: '{"count":1}', original_bytes: 12, truncated_bytes: 0 },
      }),
    ]);
  });

  it("rejects a match card through the shared command_rejected channel", () => {
    const card: ChatItem = {
      kind: "match",
      proposal: { id: "m1", path: "/etc/x", find: "foo", before: 80, after: 80, cmd: "grep ...", execution: { full_cmd: "grep ...", sentinel: "s", timeout_s: 60 } },
      at: 1,
    };
    expect(applyTerminalMutations([card], [
      { kind: "command_rejected", payload: { id: "m1", reason: "nope" } },
    ])).toEqual([
      expect.objectContaining({
        kind: "match",
        rejected: { reason: "nope" },
        result: undefined,
      }),
    ]);
  });

});
