import type { Terminal } from "@xterm/xterm";

/**
 * Row height in CSS pixels, renderer-agnostic.
 *
 * Querying `.xterm-rows > *` only works under the DomRenderer — the WebGL
 * renderer draws to a canvas and creates no row DOM. Both renderers size
 * `.xterm-screen` (a CoreBrowserTerminal-owned element, present either way)
 * to exactly rows × cell height, so derive from that.
 */
export function terminalRowHeight(terminal: Terminal, host: HTMLElement): number {
    if (terminal.rows <= 0) return 0;
    const screen = host.querySelector(".xterm-screen");
    if (!(screen instanceof HTMLElement)) return 0;
    return screen.getBoundingClientRect().height / terminal.rows;
}
