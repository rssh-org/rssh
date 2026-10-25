import { beforeEach, describe, expect, it, vi } from "vitest";
import { Terminal } from "@xterm/xterm";
import { createOutputFeeder } from "./output-feeder.ts";

beforeEach(() => vi.resetModules());

describe("alternate-screen providers", () => {
  it.each([47, 1047, 1049])("follows xterm DECSET/DECRST %s buffer switches", async (mode) => {
    const modes = await import("./alternate-screen.ts");
    const terminal = new Terminal({ allowProposedApi: true });
    const write = (text: string) => new Promise<void>((resolve) => terminal.write(text, resolve));
    modes.registerAlternateScreenProvider("target", () => terminal.buffer.active.type === "alternate");
    try {
      await write("shell prompt> ");
      expect(modes.alternateScreenActive("target")).toBe(false);
      // Real PTY output can split escape sequences across transport chunks.
      await write("\x1b[?");
      await write(`${mode}h`);
      await write("full-screen application");
      expect(modes.alternateScreenActive("target")).toBe(true);
      await write(`\x1b[?${mode}l`);
      expect(modes.alternateScreenActive("target")).toBe(false);
    } finally {
      modes.unregisterAlternateScreenProvider("target");
      terminal.dispose();
    }
  });

  it("reads live buffer changes independently for each terminal", async () => {
    const modes = await import("./alternate-screen.ts");
    let alternate = false;
    modes.registerAlternateScreenProvider("target", () => alternate);
    modes.registerAlternateScreenProvider("other", () => true);

    expect(modes.alternateScreenActive("target")).toBe(false);
    expect(modes.alternateScreenActive("other")).toBe(true);
    alternate = true;
    expect(modes.alternateScreenActive("target")).toBe(true);
    alternate = false;
    expect(modes.alternateScreenActive("target")).toBe(false);
  });

  it("drops closed terminals and uses a replacement provider on remount", async () => {
    const modes = await import("./alternate-screen.ts");
    expect(modes.alternateScreenActive("target")).toBe(false);
    modes.registerAlternateScreenProvider("target", () => true);
    modes.unregisterAlternateScreenProvider("target");
    expect(modes.alternateScreenActive("target")).toBe(false);
    modes.registerAlternateScreenProvider("target", () => true);
    modes.registerAlternateScreenProvider("target", () => false);
    expect(modes.alternateScreenActive("target")).toBe(false);
  });

  it("includes output received while an earlier flush was pending", async () => {
    const modes = await import("./alternate-screen.ts");
    const writes: (() => void)[] = [];
    const output = createOutputFeeder({ write: (_data, cb) => writes.push(cb), maxPendingBytes: 100 });
    modes.registerAlternateScreenProvider("target", () => false, output);
    output.push("first");
    const done = vi.fn();
    const pending = modes.flushTerminalOutput("target")!.then(done);
    output.push("later alternate-screen switch");
    writes.shift()!();
    await Promise.resolve();
    expect(done).not.toHaveBeenCalled();
    writes.shift()!();
    await pending;
    expect(done).toHaveBeenCalledWith(true);
    output.dispose();
  });

  it.each(["closed", "replaced", "cancelled"])("does not authorize a %s caller after parsing", async kind => {
    const modes = await import("./alternate-screen.ts");
    let parsed!: () => void;
    let needed = true;
    const output = createOutputFeeder({ write: (_data, cb) => { parsed = cb; }, maxPendingBytes: 100 });
    modes.registerAlternateScreenProvider("target", () => false, output);
    output.push("first");
    const pending = modes.flushTerminalOutput("target", () => needed);
    if (kind === "closed") modes.unregisterAlternateScreenProvider("target");
    if (kind === "replaced") modes.registerAlternateScreenProvider("target", () => false, output);
    if (kind === "cancelled") needed = false;
    parsed();
    expect(await pending).toBe(false);
    output.dispose();
  });
});
