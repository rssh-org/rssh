import { beforeEach, describe, expect, it, vi } from "vitest";
import { Terminal } from "@xterm/xterm";

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
});
