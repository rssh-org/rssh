import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Terminal } from "@xterm/xterm";
import { createOutputFeeder } from "../terminal/output-feeder.ts";

const { invokeMock, listenMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(async (_command: string, _args?: unknown): Promise<unknown> => null),
  listenMock: vi.fn(async (_event: string, _handler: (event: { payload: unknown }) => void) => vi.fn()),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

const session = { tabId: "terminal-tab", instanceId: "terminal-actor" };
const execution = { full_cmd: "echo test", sentinel: "sentinel", timeout_s: 30 };

beforeEach(() => {
  vi.resetModules();
  invokeMock.mockReset().mockResolvedValue(null);
  listenMock.mockClear();
  vi.stubGlobal("localStorage", { getItem: () => null, setItem: vi.fn() });
  vi.stubGlobal("window", globalThis);
});
afterEach(() => vi.unstubAllGlobals());

async function terminalFixture() {
  const ai = await import("./store.svelte.ts");
  const screen = await import("../terminal/alternate-screen.ts");
  const terminal = new Terminal({ allowProposedApi: true });
  const feeder = createOutputFeeder({ write: (data, done) => terminal.write(data, done), maxPendingBytes: 10000 });
  screen.registerAlternateScreenProvider(session.tabId,
    () => terminal.buffer.active.type === "alternate", feeder);
  return {
    ai, screen, terminal, feeder,
    write: (text: string) => new Promise<void>(resolve => terminal.write(text, resolve)),
    dispose: () => {
      screen.unregisterAlternateScreenProvider(session.tabId);
      feeder.dispose();
      terminal.dispose();
    },
  };
}

it.each([47, 1047, 1049])("waits for queued DECSET %s before writing a command", async mode => {
  const f = await terminalFixture();
  f.feeder.push("previous output\r\n");
  f.feeder.push(`\x1b[?${mode}h`);
  const running = f.ai.executeCommand(session, "enter", execution, "local", "target");
  try {
    await vi.waitFor(() => expect(f.terminal.buffer.active.type).toBe("alternate"));
    expect(invokeMock.mock.calls.map(([cmd]) => cmd)).toEqual(["ai_command_result"]);
    expect(invokeMock).toHaveBeenCalledWith("ai_command_result", expect.objectContaining({ exitCode: -1 }));
  } finally {
    await f.ai.submitCommand(session, "enter");
    await running;
    f.dispose();
  }
});

it("waits for queued DECRST before allowing a command at the restored prompt", async () => {
  const f = await terminalFixture();
  await f.write("\x1b[?1049hVim");
  f.feeder.push("\x1b[?1049luser@host:~$ ");
  const running = f.ai.executeCommand(session, "exit", execution, "local", "target");
  try {
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith("pty_write", expect.anything()));
    expect(invokeMock).not.toHaveBeenCalledWith("ai_command_result", expect.anything());
  } finally {
    await f.ai.submitCommand(session, "exit");
    await running;
    f.dispose();
  }
});

it("cancels while output is parsing without sending shell input or Ctrl+C", async () => {
  const ai = await import("./store.svelte.ts");
  const screen = await import("../terminal/alternate-screen.ts");
  let drained!: (ready: boolean) => void;
  const pending = new Promise<boolean>(resolve => { drained = resolve; });
  screen.registerAlternateScreenProvider(session.tabId, () => false, { flush: () => pending, pendingBytes: () => 1 });
  const running = ai.executeCommand(session, "cancel", execution, "ssh", "target");
  await ai.terminateCommand(session, "cancel");
  await running;
  expect(invokeMock.mock.calls.map(([cmd]) => cmd)).toEqual(["ai_command_result"]);
  drained(true);
  await Promise.resolve();
  expect(invokeMock.mock.calls.map(([cmd]) => cmd)).toEqual(["ai_command_result"]);
  screen.unregisterAlternateScreenProvider(session.tabId);
});

it("refuses a proposal after queued output is parsed without mounting an approval card", async () => {
  const f = await terminalFixture();
  f.ai.activateTab(session.tabId);
  invokeMock.mockImplementation(async cmd => cmd === "ai_session_start" ? {
    tab_id: session.tabId, instance_id: session.instanceId, target_id: "target",
    skill: "general", provider: "openai", model: "model", conversation_id: "conversation",
  } : null);
  await f.ai.startSession({ tabId: session.tabId, targetKind: "ssh", targetId: "target",
    skill: "general", provider: "openai", model: "model", lease: f.ai.captureSessionLease(session.tabId) });
  const proposed = listenMock.mock.calls.find(([event]) => event === `ai:command_proposed:${session.tabId}`)![1];
  invokeMock.mockClear();
  f.feeder.push("output\r\n");
  f.feeder.push("\x1b[?1049h");
  try {
    proposed({ payload: { id: "arrival", kind: "run_command", cmd: "echo test", execution } });
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith("ai_command_result",
      expect.objectContaining({ toolCallId: "arrival", exitCode: -1 })));
    expect(invokeMock.mock.calls.map(([cmd]) => cmd)).toEqual(["ai_command_result"]);
  } finally {
    await f.ai.stopSession(session.tabId);
    f.dispose();
  }
});
