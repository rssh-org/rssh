import { readFileSync } from "node:fs";
import { join } from "node:path";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { describe, expect, it, vi } from "vitest";
import { errMsg, setLocale } from "../i18n/index.svelte.ts";

const source = readFileSync(join(process.cwd(), "frontend/lib/components/TerminalPane.svelte"), "utf8");
const functions = [
  source.slice(source.indexOf("function sendText("), source.indexOf("async function copySelection(")),
  source.slice(source.indexOf("function wireSessionInput("), source.indexOf("/** Full connect cycle:")),
  source.slice(source.indexOf("function writeControl("), source.indexOf("function sendControlArrow(")),
].join("\n");
const code = ts.transpileModule(functions, {
  compilerOptions: { target: ts.ScriptTarget.ES2022 },
}).outputText;

describe("terminal input encoding failures", () => {
  it.each(["typing", "paste", "snippet", "control"])("reports rejected %s input and keeps the session usable", async (path) => {
    setLocale("en");
    const error = '__rssh_err__|{"code":"terminal_encoding_unmappable","params":{"encoding":"gbk"}}';
    let onData: (text: string) => void = () => {};
    // A controlled rejected request lets the test verify that the input path
    // attaches an error handler without leaking an unhandled rejection.
    const rejected = { catch: vi.fn((handler: (error: unknown) => void) => handler(error)) };
    const invoke = vi.fn().mockReturnValueOnce(rejected).mockResolvedValue(undefined);
    const toast = { error: vi.fn() };
    const context = {
      invoke, toast, errMsg, TextEncoder, console,
      destroyed: false, disconnected: false, sessionId: "ssh-session", streamOpts: undefined,
      writeCmd: "ssh_write", resizeCmd: "ssh_resize", tabType: "ssh",
      dataDisposable: undefined, resizeDisposable: undefined,
      maybeReleaseBacklog: vi.fn(), processInput: (text: string) => text,
      terminal: {
        modes: { bracketedPasteMode: true },
        onData: (handler: typeof onData) => { onData = handler; return { dispose() {} }; },
        onResize: () => ({ dispose() {} }),
      },
    };
    const controls = runInNewContext(code + "\n({ sendText, pasteText, wireSessionInput, writeControl })", context);
    controls.wireSessionInput("ssh-session");
    const send = path === "typing" ? onData : path === "paste" ? controls.pasteText
      : path === "snippet" ? controls.sendText : controls.writeControl;

    send("😀");
    expect(rejected.catch).toHaveBeenCalledOnce();
    expect(toast.error).toHaveBeenCalledExactlyOnceWith(errMsg(error));
    expect(toast.error.mock.calls[0][0]).not.toContain("__rssh_err__");
    expect(context.disconnected).toBe(false);
    send("ls\r");
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(invoke.mock.calls[1][0]).toBe("ssh_write");
    expect(invoke.mock.calls[1][1].sessionId).toBe("ssh-session");
  });
});
