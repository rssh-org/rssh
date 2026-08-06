/**
 * Shell prompt recognition for terminal presentation features.
 *
 * WindTerm takes the pragmatic route: shell-family regular expressions, not
 * OCR and not a second command-block protocol. We do the same. Detection is
 * deliberately anchored to column zero and callers should only inspect the
 * first visual row of a command block; that keeps ordinary output out of the
 * heuristic's input.
 */

export interface PromptMatch {
  /** UTF-16 offset immediately after the prompt marker, before command spacing. */
  end: number;
}

const PROMPT_PATTERNS: ReadonlyArray<RegExp> = [
  // PowerShell: "PS C:\\Users\\alice>" and cross-platform "PS /home/alice>".
  /^PS(?:\s+[^>\r\n]{0,200})?>/i,
  // cmd.exe: drive paths and UNC paths.
  /^(?:[A-Za-z]:\\|\\\\)[^>\r\n]{0,200}>/,
  // Unix user@host prompts, optionally preceded by venv/context and followed
  // by either :path or fish-style " path".
  /^(?:(?:\([^)\r\n]{1,80}\)|\[[^\]\r\n]{1,120}\])\s*)*[^\s@:\r\n]+@[^\s:\r\n]+(?:(?::|\s+)[^#$%>\r\n]{0,160})?[#$%>]/,
  // Traditional bracket prompt: "[root@host path]#" or "[ctx]$".
  /^\[[^\]\r\n]{1,160}\][#$%>]/,
  // macOS' historical default: "host:directory user$".
  /^[A-Za-z0-9._-]{1,80}:[^#$%>\r\n]{0,120}\s+[A-Za-z0-9._-]{1,80}[#$%>]/,
  // Powerline: the final segment separator marks the end of the prompt. Keep
  // this greedy so a multi-segment prompt is fully redacted.
  /^[^\r\n]{0,200}[\uE0B0\uE0B1]/,
  // Starship / oh-my-zsh symbolic prompts, alone or after a short status/path.
  /^(?:[^❯➜➤λ\r\n]{1,160}\s)?[❯➜➤λ](?=\s|$)/,
  // Versioned shell prompts such as "bash-5.2$".
  /^[A-Za-z][A-Za-z0-9._-]{0,60}[#$%>](?=\s|$)/,
  // Minimal POSIX prompts.
  /^[#$%>](?=\s|$)/,
];

export function detectPrompt(text: string): PromptMatch | null {
  for (const pattern of PROMPT_PATTERNS) {
    const match = pattern.exec(text);
    if (match) return { end: match[0].length };
  }
  return null;
}
