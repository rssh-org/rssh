/**
 * Plugin bridge protocol v1 — the ONLY channel between a plugin iframe and
 * the rssh host. postMessage both ways; the host brokers every request to a
 * Tauri command and never hands the iframe `invoke` access.
 *
 * plugin → host: { v:1, id, cmd:"exec", payload:{ command, timeoutMs? } }
 *              | { v:1, id, cmd:"size", payload:{ width?, height? } } (notification, no reply)
 * host → plugin: { v:1, id, ok:true,  result:{stdout,stderr,exitCode} }
 *              | { v:1, id, ok:false, error:{code,message} }
 * host → plugin events: { v:1, evt:"hello",      payload:{apiVersion, theme} }
 *                       | { v:1, evt:"visibility", payload:{visible} }
 *
 * `theme` carries the app's design tokens (see THEME_TOKENS) so plugin UIs
 * match rssh without hardcoding colors — the sandbox hides the host
 * stylesheet from the iframe, this is the only way colors travel.
 *
 * The iframe runs sandboxed (opaque origin), so origin checks are useless —
 * the host validates `event.source === iframe.contentWindow` instead.
 */

export const PLUGIN_API_VERSION = 1;

/** Longest command a plugin may run (mirrors the Rust-side cap). */
export const MAX_COMMAND_LENGTH = 4096;

/** Concurrent in-flight execs per plugin iframe. */
export const MAX_CONCURRENT_EXEC = 4;

/** Upper bound for a plugin-reported dimension (px). */
export const MAX_REPORTED_SIZE = 8192;

export interface ExecRequest {
  command: string;
  timeoutMs?: number;
}

export interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number | null;
}

/** Content size a plugin reports so the host can size its container. The
 *  sandbox blocks the host from measuring iframe content — only the plugin
 *  knows its natural height (side cards) or width (strip segments). */
export interface SizeReport {
  width?: number;
  height?: number;
}

/** plugin → host message shape (after validation). */
export type PluginRequest =
  | { id: string; cmd: "exec"; payload: ExecRequest }
  | { id: string; cmd: "size"; payload: SizeReport };

export function isPluginRequest(value: unknown): value is PluginRequest {
  if (typeof value !== "object" || value === null) return false;
  const m = value as Record<string, unknown>;
  if (m.v !== PLUGIN_API_VERSION) return false;
  if (typeof m.id !== "string" || m.id.length === 0 || m.id.length > 64) return false;
  if (m.cmd === "exec") return isExecPayload(m.payload);
  if (m.cmd === "size") return isSizePayload(m.payload);
  return false;
}

function isExecPayload(p: unknown): boolean {
  if (typeof p !== "object" || p === null) return false;
  const req = p as Record<string, unknown>;
  if (typeof req.command !== "string" || req.command.length === 0) return false;
  if (req.command.length > MAX_COMMAND_LENGTH) return false;
  // Finite and positive: NaN/Infinity serialize to null at the invoke boundary,
  // and a non-positive timeout is meaningless (the backend clamps the rest).
  // typeof narrows the unknown so the comparison type-checks; Number.isFinite
  // never throws or coerces, so symbols/bigints fail the typeof check instead.
  const timeoutMs = req.timeoutMs;
  if (timeoutMs !== undefined && (typeof timeoutMs !== "number" || !Number.isFinite(timeoutMs) || timeoutMs <= 0))
    return false;
  return true;
}

/** Size notifications carry at least one sane dimension (finite, capped). */
function isSizePayload(p: unknown): boolean {
  if (typeof p !== "object" || p === null) return false;
  const rep = p as Record<string, unknown>;
  const ok = (v: unknown) =>
    typeof v === "number" && Number.isFinite(v) && v > 0 && v <= MAX_REPORTED_SIZE;
  if (rep.width === undefined && rep.height === undefined) return false;
  if (rep.width !== undefined && !ok(rep.width)) return false;
  if (rep.height !== undefined && !ok(rep.height)) return false;
  return true;
}

/** host → plugin response for a request id. */
export function execResponseOk(id: string, result: ExecResult) {
  return { v: PLUGIN_API_VERSION, id, ok: true, result };
}

export function execResponseErr(id: string, code: string, message: string) {
  return { v: PLUGIN_API_VERSION, id, ok: false, error: { code, message } };
}

/** host → plugin event frames. */
export function helloEvent(theme?: ThemeTokens) {
  return {
    v: PLUGIN_API_VERSION,
    evt: "hello",
    payload: { apiVersion: PLUGIN_API_VERSION, theme },
  };
}

export function visibilityEvent(visible: boolean) {
  return { v: PLUGIN_API_VERSION, evt: "visibility", payload: { visible } };
}

// ── Theme tokens ──────────────────────────────────────────────────────────

/** Design tokens forwarded to plugins (hello event) so their UI matches the
 *  host theme. Only these travel across the bridge; anything else stays
 *  host-private. Plugins reference them by the same var(--name). */
export const THEME_TOKENS = [
  "--bg",
  "--surface",
  "--divider",
  "--text",
  "--text-sub",
  "--text-dim",
  "--accent",
  "--accent-soft",
  "--error",
  "--success",
  "--warning",
  "--magenta",
  "--radius-sm",
  "--term-font",
] as const;

export type ThemeTokens = Partial<Record<(typeof THEME_TOKENS)[number], string>>;

/** Read current token values off a computed style — pass
 *  `getComputedStyle(document.documentElement)`. Empty values are dropped. */
export function readThemeTokens(
  style: Pick<CSSStyleDeclaration, "getPropertyValue">,
): ThemeTokens {
  const tokens: ThemeTokens = {};
  for (const name of THEME_TOKENS) {
    const value = style.getPropertyValue(name).trim();
    if (value) tokens[name] = value;
  }
  return tokens;
}

/** Preview documents run no bridge — the manager passes tokens via the URL
 *  fragment instead (`…preview.html#rssh-theme=<encoded json>`), and the
 *  preview applies them itself. */
export function withThemeFragment(url: string, tokens: ThemeTokens): string {
  return `${url.split("#")[0]}#rssh-theme=${encodeURIComponent(JSON.stringify(tokens))}`;
}
