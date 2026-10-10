import { readFileSync } from "node:fs";
import { join } from "node:path";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { describe, expect, it, vi } from "vitest";
import { errMsg, setLocale } from "../i18n/index.svelte.ts";
import { createReservedSessionAttempt } from "../terminal/reserved-session-attempt.ts";

const source = readFileSync(join(process.cwd(), "src/lib/components/TerminalPane.svelte"), "utf8");
const start = source.indexOf("async function connectAndWire()");
const end = source.indexOf("function processInput", start);
const connectCode = ts.transpileModule(source.slice(start, end), {
  compilerOptions: { target: ts.ScriptTarget.ES2022 },
}).outputText;

describe("terminal connection errors", () => {
  it.each([
    { tabType: "ssh", code: "ssh_connect_timeout", params: { host: "test.invalid", port: 22, secs: 10 }, message: "test.invalid:22 connect timed out (10s)" },
    { tabType: "local", code: "pty_op_failed", params: { err: "shell unavailable" }, message: "PTY operation failed: shell unavailable" },
  ])("renders a coded $tabType error as a readable message", async ({ tabType, code, params, message }) => {
    setLocale("en");
    const error = `__rssh_err__|${JSON.stringify({ code, params })}`;
    const terminal = { write: vi.fn(), cols: 80, rows: 24 };
    const reportInitialConnectionFailure = vi.fn();
    const invoke = vi.fn().mockRejectedValue(error);
    const reservedSessionAttempt = createReservedSessionAttempt({
      makeId: () => "new-session",
      wireEvents: async () => () => {},
      close: async () => {},
    });
    const connect = runInNewContext(connectCode + "\nconnectAndWire", {
      destroyed: false, connectGeneration: 0, dataDisposable: undefined, resizeDisposable: undefined,
      reservedSessionAttempt, clearSshPromptUi: () => {}, TextDecoder,
      tabType, isLocal: tabType === "local", isPtyConnector: false,
      tabId: "new-tab", meta: { profileId: "profile-1" },
      terminal, invoke, reportInitialConnectionFailure, errMsg,
    }) as () => Promise<boolean>;

    await expect(connect()).resolves.toBe(false);
    expect(invoke).toHaveBeenCalledWith(tabType === "ssh" ? "ssh_connect" : "pty_spawn", expect.objectContaining({ sessionId: "new-session" }));
    expect(reportInitialConnectionFailure).toHaveBeenCalledWith(error);
    const output = terminal.write.mock.calls.map(([text]) => text).join("");
    expect(output).toContain(message);
    expect(output).not.toContain("__rssh_err__");
  });
});
