import { expect, it } from "vitest";
import { Terminal } from "@xterm/xterm";
import { createOutputFeeder } from "./output-feeder.ts";
import { resetTerminalConnection } from "./connection-reset.ts";

it.each([0, 47, 1047, 1049])("reconnects from mode %s without losing normal scrollback", async mode => {
  const terminal = new Terminal({ allowProposedApi: true, rows: 3 });
  const feeder = createOutputFeeder({ write: (data, cb) => terminal.write(data, cb), maxPendingBytes: 10000 });
  const lifetime = new AbortController();
  const write = (text: string) => new Promise<void>(resolve => terminal.write(text, resolve));
  const history = () => Array.from({ length: terminal.buffer.normal.length }, (_, i) =>
    terminal.buffer.normal.getLine(i)!.translateToString(true));
  try {
    await write("one\r\ntwo\r\nthree\r\nfour\r\n");
    const before = history();
    // The old peer's final bytes may still be queued when reconnect starts.
    feeder.push("\x1b[?2004h\x1b[?1h");
    if (mode) feeder.push(`\x1b[?${mode}h\x1b[1;1HVim`);
    feeder.push("\x1b]0;incomplete title");
    expect(await resetTerminalConnection(terminal, feeder, lifetime.signal)).toBe(true);
    expect(terminal.buffer.active.type).toBe("normal");
    expect(history()).toEqual(before);
    expect(terminal.modes.bracketedPasteMode).toBe(false);
    expect(terminal.modes.applicationCursorKeysMode).toBe(false);
    await write("fresh shell$ ");
    expect(terminal.buffer.active.type).toBe("normal");
    expect(history()).toEqual([...before.slice(0, -1), "fresh shell$ "]);
  } finally {
    lifetime.abort();
    feeder.dispose();
    terminal.dispose();
  }
});

it("settles a pending reset when the terminal is destroyed", async () => {
  const lifetime = new AbortController();
  let written!: () => void;
  const terminal = {
    cols: 80,
    buffer: { normal: { cursorX: 0, cursorY: 0 } },
    write: (_data: string, cb: () => void) => { written = cb; },
  };
  const feeder = createOutputFeeder({ write: (_data, cb) => cb(), maxPendingBytes: 100 });
  const reset = resetTerminalConnection(terminal, feeder, lifetime.signal);
  await Promise.resolve();
  lifetime.abort();
  expect(await reset).toBe(false);
  written();
  feeder.dispose();
});

it("keeps a full-width last line when the fresh prompt wraps", async () => {
  const terminal = new Terminal({ allowProposedApi: true, cols: 5, rows: 3 });
  const feeder = createOutputFeeder({ write: (data, cb) => terminal.write(data, cb), maxPendingBytes: 100 });
  try {
    feeder.push("12345");
    await resetTerminalConnection(terminal, feeder, new AbortController().signal);
    await new Promise<void>(resolve => terminal.write("P", resolve));
    expect(terminal.buffer.normal.getLine(0)!.translateToString(true)).toBe("12345");
    expect(terminal.buffer.normal.getLine(1)!.translateToString(true)).toBe("P");
  } finally {
    feeder.dispose();
    terminal.dispose();
  }
});
