import { invoke } from "@tauri-apps/api/core";

/** Serial features belong to the host API, not the saved connection profile. */
export interface SerialCapabilities {
  flowControl: boolean;
  xany: boolean;
  signals: boolean;
  /** Empty means the host accepts a custom rate. */
  baudRates: number[];
}

type LoadState =
  | { kind: "idle" | "loading" }
  | { kind: "ready"; value: SerialCapabilities }
  | { kind: "error"; error: unknown };

let _state = $state<LoadState>({ kind: "idle" });
let pending: Promise<void> | undefined;

export function state(): LoadState { return _state; }
export function capabilities(): SerialCapabilities | null {
  return _state.kind === "ready" ? _state.value : null;
}

/** Failed reads remain visible to callers; the next explicit load retries. */
export function load(): Promise<void> {
  if (_state.kind === "ready") return Promise.resolve();
  if (pending) return pending;
  _state = { kind: "loading" };
  pending = invoke<SerialCapabilities>("serial_get_capabilities")
    .then((value) => { _state = { kind: "ready", value }; })
    .catch((error) => { _state = { kind: "error", error }; })
    .finally(() => { pending = undefined; });
  return pending;
}
