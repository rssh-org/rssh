import { invoke } from "@tauri-apps/api/core";
import { onRuntimeCapabilitiesChanged } from "../ipc-shim.ts";

/** Host performance defaults are independent of layout and input hardware. */
export interface TerminalRuntimePolicy {
  imageStorageLimitMb: number;
  imagePixelLimit: number;
  outputBacklogLimitBytes: number;
  gpuRenderDefault: boolean;
}

/** The host reports implemented capabilities, independently of screen shape. */
export interface RuntimeCapabilities {
  localPty: boolean;
  serial: boolean;
  localDiscovery: boolean;
  sshAgent: boolean;
  defaultKeyFiles: boolean;
  cliInstall: boolean;
  multiWindow: boolean;
  windowControls: boolean;
  windowPin: boolean;
  fileMultiSelect: boolean;
  directoryTransfer: boolean;
  plugins: boolean;
  nativeClipboard: boolean;
}

export interface RuntimeResponse extends RuntimeCapabilities {
  terminalPolicy: TerminalRuntimePolicy;
}

let _capabilities = $state<Readonly<RuntimeCapabilities>>({
  localPty: false,
  serial: false,
  localDiscovery: false,
  sshAgent: false,
  defaultKeyFiles: false,
  cliInstall: false,
  multiWindow: false,
  windowControls: false,
  windowPin: false,
  fileMultiSelect: false,
  directoryTransfer: false,
  plugins: false,
  nativeClipboard: false,
});
let _terminalPolicy = $state<Readonly<TerminalRuntimePolicy> | null>(null);
let _loaded = $state(false);
let pending: Promise<void> | undefined;
let observingCapabilities = false;

export function capabilities(): Readonly<RuntimeCapabilities> { return _capabilities; }
export function loaded(): boolean { return _loaded; }
export function terminalPolicy(): Readonly<TerminalRuntimePolicy> {
  if (!_terminalPolicy) throw new Error("Host terminal runtime policy is not loaded");
  return _terminalPolicy;
}

/** App mounts resource-owning components only after this succeeds. */
export function load(): Promise<void> {
  observeCapabilities();
  if (_loaded) return Promise.resolve();
  return requestCapabilities();
}

function requestCapabilities(): Promise<void> {
  pending ??= invoke<RuntimeResponse>("get_runtime_capabilities")
    .then(({ terminalPolicy, ...capabilities }) => {
      if (!terminalPolicy) throw new Error("Host did not provide a terminal runtime policy");
      _capabilities = capabilities;
      _terminalPolicy = terminalPolicy;
      _loaded = true;
    })
    .finally(() => { pending = undefined; });
  return pending;
}

function observeCapabilities(): void {
  if (observingCapabilities) return;
  observingCapabilities = true;
  // Refresh capabilities without unmounting active sessions.
  onRuntimeCapabilitiesChanged(() => {
    // A failed first load belongs to App's retry flow, which also starts the
    // remaining application services. A capability change must not bypass it.
    const initialized = pending ?? (_loaded ? Promise.resolve() : undefined);
    if (!initialized) return;
    void initialized.then(requestCapabilities)
      .catch((error) => console.warn("[runtime] host capability refresh failed:", error));
  });
}
