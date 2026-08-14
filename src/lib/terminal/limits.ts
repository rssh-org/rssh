/**
 * Terminal history limits shared by xterm and command-block folding.
 *
 * Keep one explicit scrollback value instead of relying on xterm's implicit
 * default.  An auto-folded block retains at most the history that can be put
 * back without exceeding that same budget:
 *
 *   visible block rows + cached folded rows < xterm scrollback
 */
export const TERMINAL_SCROLLBACK_LINES = 1_000;

export const COMMAND_BLOCK_MAX_LINES_DEFAULT = 50;
export const COMMAND_BLOCK_MAX_LINES_MIN = 10;
// Leave at least 99 cached rows even at the largest user-selectable value.
export const COMMAND_BLOCK_MAX_LINES_MAX = 900;

export function normalizeCommandBlockMaxLines(value: unknown): number {
  if (value === null || value === undefined || value === "") {
    return COMMAND_BLOCK_MAX_LINES_DEFAULT;
  }
  const parsed = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(parsed)) return COMMAND_BLOCK_MAX_LINES_DEFAULT;
  return Math.max(
    COMMAND_BLOCK_MAX_LINES_MIN,
    Math.min(COMMAND_BLOCK_MAX_LINES_MAX, Math.trunc(parsed)),
  );
}

export function commandBlockFoldCacheLines(maxVisibleLines: number): number {
  return TERMINAL_SCROLLBACK_LINES - normalizeCommandBlockMaxLines(maxVisibleLines) - 1;
}

// ── Flood backlog ──
/** Pending bytes above which the backlog badge becomes visible. */
export const BACKLOG_INDICATOR_BYTES = 64 * 1024;
/** Ctrl+C only releases the backlog above this much pending data. */
export const BACKLOG_DROP_TRIGGER_BYTES = 256 * 1024;
/** Silence window that ends a quiescent drop after Ctrl+C. */
export const BACKLOG_QUIESCENCE_MS = 150;
/** Feeder queue cap: drop-oldest beyond this (desktop / mobile). */
export const BACKLOG_MAX_PENDING_BYTES = 128 * 1024 * 1024;
export const BACKLOG_MAX_PENDING_BYTES_MOBILE = 32 * 1024 * 1024;
