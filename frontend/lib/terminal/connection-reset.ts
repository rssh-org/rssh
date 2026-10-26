import type { OutputFeeder } from "./output-feeder.ts";

/** Cancel a partial escape sequence, leave the alternate buffer and soft-reset
 * input modes. RIS/terminal.reset() would discard the user's normal history. */
const CONNECTION_RESET = "\x18\x1b[?1047l\x1b[!p";

export async function resetTerminalConnection(
  terminal: {
    cols: number;
    buffer: { normal: { cursorX: number; cursorY: number } };
    write(data: string, callback: () => void): void;
  },
  output: OutputFeeder,
  signal: AbortSignal,
): Promise<boolean> {
  // The caller has detached the old peer. Its queued DECSET must finish before
  // the reset, otherwise it could put the replacement shell back in alt mode.
  if (!await output.flush() || signal.aborted) return false;
  return new Promise(resolve => {
    const abort = () => resolve(false);
    signal.addEventListener("abort", abort, { once: true });
    // Local banners bypass the feeder. Sample the cursor only after these
    // direct writes have parsed, using xterm's own write queue as the barrier.
    terminal.write("", () => {
      if (signal.aborted) return;
      // 1049 restores a saved cursor that may belong to an earlier application;
      // 47/1047 inherit the alternate cursor. Preserve the normal position so a
      // fresh prompt cannot overwrite earlier output.
      const { cursorX, cursorY } = terminal.buffer.normal;
      const cursor = `\x1b[${cursorY + 1};${Math.min(cursorX + 1, terminal.cols)}H`;
      const wrap = cursorX >= terminal.cols ? "\r\n" : "";
      terminal.write(`${CONNECTION_RESET}${cursor}${wrap}`, () => {
        signal.removeEventListener("abort", abort);
        resolve(!signal.aborted);
      });
    });
  });
}
