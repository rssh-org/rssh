import { readFileSync } from "node:fs";
import { join } from "node:path";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { describe, expect, it, vi } from "vitest";
import { errMsg, setLocale } from "../i18n/index.svelte.ts";
import { createReservedSessionAttempt } from "../terminal/reserved-session-attempt.ts";
import { resetTerminalConnection } from "../terminal/connection-reset.ts";
import { decodeStreamChunk } from "../terminal/stream-decode.ts";

const source = readFileSync(join(process.cwd(), "frontend/lib/components/TerminalPane.svelte"), "utf8");
const start = source.indexOf("async function connectAndWire()");
const end = source.indexOf("function processInput", start);
const connectCode = ts.transpileModule(source.slice(start, end), {
  compilerOptions: { target: ts.ScriptTarget.ES2022 },
}).outputText;
const subscriptionStart = source.indexOf("async function createSessionEventSubscription(");
const subscriptionEnd = source.indexOf("const reservedSessionAttempt =", subscriptionStart);
const subscriptionCode = ts.transpileModule(source.slice(subscriptionStart, subscriptionEnd), {
  compilerOptions: { target: ts.ScriptTarget.ES2022 },
}).outputText;

describe("terminal connection errors", () => {
  it.each([
    { tabType: "ssh", code: "ssh_connect_timeout", params: { host: "test.invalid", port: 22, secs: 10 }, message: "test.invalid:22 connect timed out (10s)" },
    { tabType: "local", code: "pty_op_failed", params: { err: "shell unavailable" }, message: "PTY operation failed: shell unavailable" },
  ])("renders a coded $tabType error as a readable message", async ({ tabType, code, params, message }) => {
    setLocale("en");
    const error = `__rssh_err__|${JSON.stringify({ code, params })}`;
    const terminal = {
      write: vi.fn((_data: string, callback?: () => void) => callback?.()),
      cols: 80, rows: 24, buffer: { normal: { cursorX: 0, cursorY: 0 } },
    };
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
      outputFeeder: { flush: async () => true }, resetTerminalConnection,
      terminalLifetime: new AbortController(),
    }) as () => Promise<boolean>;

    await expect(connect()).resolves.toBe(false);
    expect(invoke).toHaveBeenCalledWith(tabType === "ssh" ? "ssh_connect" : "pty_spawn", expect.objectContaining({ sessionId: "new-session" }));
    expect(reportInitialConnectionFailure).toHaveBeenCalledWith(error);
    const output = terminal.write.mock.calls.map(([text]) => text).join("");
    expect(output).toContain(message);
    expect(output).not.toContain("__rssh_err__");
  });
});

describe("terminal shell probe wiring", () => {
  it.each(["ssh", "local", "docker_exec", "kubectl_exec", "aws_ssm"])("asks the shared backend gate after connecting %s", async (tabType) => {
    const ensureRemoteShellProbe = vi.fn().mockResolvedValue(false);
    const reservedSessionAttempt = createReservedSessionAttempt({
      makeId: () => "new-session", wireEvents: async () => () => {}, close: async () => {},
    });
    const context = {
      destroyed: false, connectGeneration: 0, dataDisposable: undefined, resizeDisposable: undefined,
      reservedSessionAttempt, clearSshPromptUi: () => {}, TextDecoder,
      tabType, isSsh: tabType === "ssh", isLocal: tabType === "local",
      isPtyConnector: ["docker_exec", "kubectl_exec", "aws_ssm"].includes(tabType),
      tabId: "new-tab", meta: { profileId: "profile-1", connectorSpec: "{}" },
      terminal: { cols: 80, rows: 24 }, invoke: vi.fn(async () => "new-session"),
      outputFeeder: undefined, wireSessionInput: vi.fn(), requestAnimationFrame: vi.fn(),
      ai: { ensureRemoteShellProbe },
    };
    const connect = runInNewContext(connectCode + "\nconnectAndWire", context) as () => Promise<boolean>;
    expect(await connect()).toBe(true);
    expect(ensureRemoteShellProbe).toHaveBeenCalledExactlyOnceWith("new-session", tabType === "ssh" ? "ssh" : "pty");
  });

  it("prepares the collector before subscribing and feeds the existing data callback without changing output", async () => {
    const prepareRemoteShellProbe = vi.fn();
    const observeRemoteShellOutput = vi.fn();
    const writeRawOutput = vi.fn();
    const callbacks = new Map<string, (event: { payload: string }) => void>();
    const listen = vi.fn(async (event: string, callback: (event: { payload: string }) => void) => {
      expect(prepareRemoteShellProbe).toHaveBeenCalledExactlyOnceWith("new-session", "pty");
      callbacks.set(event, callback);
      return vi.fn();
    });
    const subscribe = runInNewContext(subscriptionCode + "\ncreateSessionEventSubscription", {
      listen, dataEvent: "pty:data", closeEvent: "pty:close",
      isSsh: false, isLocal: false, isPtyConnector: true, tabType: "aws_ssm",
      acceptsSessionEvent: () => true, decodeStreamChunk, streamOpts: undefined, writeRawOutput,
      ai: { prepareRemoteShellProbe, observeRemoteShellOutput, cancelRemoteShellProbe: vi.fn() },
    }) as (sid: string) => Promise<() => void>;
    const subscription = subscribe("new-session");
    expect(prepareRemoteShellProbe).toHaveBeenCalledOnce();
    const release = await subscription;
    callbacks.get("pty:data:new-session")!({ payload: btoa("early probe output\r\n") });
    const bytes = new TextEncoder().encode("early probe output\r\n");
    expect(observeRemoteShellOutput).toHaveBeenCalledExactlyOnceWith("new-session", "pty", bytes);
    expect(writeRawOutput).toHaveBeenCalledExactlyOnceWith(bytes);
    expect(observeRemoteShellOutput.mock.calls[0][2]).toBe(writeRawOutput.mock.calls[0][0]);
    expect(listen.mock.calls.map(([event]) => event)).toEqual(["pty:data:new-session", "pty:close:new-session"]);
    release();
  });

  it("cleans the prepared collector if event subscription fails before connect", async () => {
    const cancelRemoteShellProbe = vi.fn();
    const stop = vi.fn();
    const listen = vi.fn().mockResolvedValueOnce(stop).mockRejectedValueOnce(new Error("listen failed"));
    const subscribe = runInNewContext(subscriptionCode + "\ncreateSessionEventSubscription", {
      listen, dataEvent: "pty:data", closeEvent: "pty:close",
      isSsh: false, isLocal: false, isPtyConnector: true, tabType: "aws_ssm",
      ai: { cancelRemoteShellProbe, prepareRemoteShellProbe: vi.fn() },
    }) as (sid: string) => Promise<() => void>;
    await expect(subscribe("failed-session")).rejects.toThrow("listen failed");
    expect(cancelRemoteShellProbe).toHaveBeenCalledExactlyOnceWith("failed-session", "pty");
    expect(stop).toHaveBeenCalledOnce();
  });

  it("cancels the collector when a PTY subscription is released on close or reconnect", async () => {
    const cancelRemoteShellProbe = vi.fn();
    const stop = vi.fn();
    const subscribe = runInNewContext(subscriptionCode + "\ncreateSessionEventSubscription", {
      listen: vi.fn(async () => stop), dataEvent: "pty:data", closeEvent: "pty:close",
      isSsh: false, isLocal: false, isPtyConnector: true, tabType: "aws_ssm",
      ai: { cancelRemoteShellProbe, prepareRemoteShellProbe: vi.fn() },
    }) as (sid: string) => Promise<() => void>;
    const release = await subscribe("old-session");
    release();
    expect(cancelRemoteShellProbe).toHaveBeenCalledExactlyOnceWith("old-session", "pty");
    expect(stop).toHaveBeenCalledTimes(2);
  });
});
