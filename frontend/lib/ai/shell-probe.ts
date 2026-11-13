/** Remote-shell probe shared by SSH and connector-backed PTYs. */
import type { ShellKind } from "./types.ts";
import { stripAnsi } from "./pty-output.ts";

/** Map one evaluated output line's two captured groups to a shell family.
 * null means the response completed, but its shell signature is unsupported. */
export function classifyShell(psed: string, dollar: string): ShellKind | null {
  // PS 5+ exposes $PSEdition = 'Desktop' (Windows PowerShell) or 'Core' (PS 7).
  if (psed === "Desktop" || psed === "Core") return "powershell";
  // cmd.exe expands no `$`-style vars — the whole token comes back literal.
  if (psed === "$PSEdition" && dollar === "$$") return "cmd";
  // POSIX: $PSEdition expands to empty, $$ is the shell PID (digits).
  if (psed === "" && /^\d+$/.test(dollar)) return "posix";
  return null;
}

export interface ShellProbe {
  command: string;
  /** null means no complete response yet; { shell: null } is a completed,
   * unsupported signature. Callers can distinguish readiness from detection. */
  classify(buffer: string): { shell: ShellKind | null } | null;
}

export function createShellProbe(sessionId: string): ShellProbe {
  // The reserved UUID identifies this connection attempt on both sides. The
  // backend enqueues this command; the frontend only recognizes its response.
  const marker = `RSSH_SHELL_${sessionId.replaceAll("-", "")}`;
  const command = `echo ${marker}=$PSEdition=$$=END`;
  // The echoed input starts with `echo ` (possibly after a prompt), whereas
  // evaluated output starts with the marker. Require the entire LF/CRLF line:
  // a split chunk or a matching substring is not a completed response. Avoid
  // lookbehind so older supported WebKit versions can parse this module.
  const response = new RegExp(`(?:^|\n)${marker}=([^=\r\n]*)=([^\r\n]*)=END\r?\n`);
  return {
    command,
    classify(buffer) {
      // Normalize the accumulated private buffer so split control sequences
      // stay intact. The terminal still receives the original bytes unchanged.
      const match = response.exec(stripAnsi(buffer));
      return match ? { shell: classifyShell(match[1], match[2]) } : null;
    },
  };
}
