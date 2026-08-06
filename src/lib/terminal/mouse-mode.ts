import type { Terminal } from "@xterm/xterm";

// Reset only xterm's mouse protocol and encoding. Other DEC private modes
// (alternate buffer, bracketed paste, cursor keys, focus events) belong to the
// foreground program and must not be guessed or rewritten here.
const RESET_MOUSE_TRACKING = "\x1b[?9;1000;1002;1003;1006;1016l";

export function resetMouseTracking(term: Pick<Terminal, "write">, done?: () => void): void {
    term.write(RESET_MOUSE_TRACKING, done);
}
