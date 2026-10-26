/**
 * Per-tab bracketed-paste mode bridge.
 *
 * xterm.js tracks DECSET 2004 (bracketed paste) from the remote shell: when
 * the shell emits `\x1b[?2004h`, `terminal.modes.bracketedPasteMode` goes
 * true and xterm wraps USER pastes with `\x1b[200~` / `\x1b[201~`. rssh's own
 * AI-driven paste (store.executeCommand) writes the PTY directly and bypasses
 * xterm, so it has to read this mode and wrap itself — exactly what the
 * terminal's manual `pasteText` already does.
 *
 * This is a leaf module (no imports) so the AI store can read it without a
 * circular dependency on the app store (which already imports the AI store).
 * TerminalPane registers a live provider per tab; executeCommand reads it at
 * paste time. Shells that never enable bracketed paste (dash / POSIX sh /
 * cmd.exe) and raw-device transports (serial / telnet) simply report false,
 * so the paste is sent raw — no per-shell / per-platform branching needed.
 */
const _providers = new Map<string, () => boolean>();

export function registerBracketedPasteProvider(tabId: string, read: () => boolean): void {
  _providers.set(tabId, read);
}

export function unregisterBracketedPasteProvider(tabId: string): void {
  _providers.delete(tabId);
}

/** Live bracketed-paste mode for a tab's terminal — false if no terminal is
 *  registered for the tab or the shell hasn't enabled it. */
export function bracketedPasteEnabled(tabId: string): boolean {
  return _providers.get(tabId)?.() ?? false;
}
