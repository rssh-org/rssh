import { invoke } from "@tauri-apps/api/core";

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
});
let _loaded = $state(false);
let pending: Promise<void> | undefined;
let observingHost = false;

export function capabilities(): Readonly<RuntimeCapabilities> { return _capabilities; }
export function loaded(): boolean { return _loaded; }

/** App mounts resource-owning components only after this succeeds. */
export function load(): Promise<void> {
  observeHost();
  if (_loaded) return Promise.resolve();
  return requestCapabilities();
}

function requestCapabilities(): Promise<void> {
  pending ??= invoke<RuntimeCapabilities>("get_runtime_capabilities")
    .then((value) => {
      _capabilities = value;
      _loaded = true;
    })
    .finally(() => { pending = undefined; });
  return pending;
}

function observeHost(): void {
  if (observingHost || typeof window === "undefined" || !window.__RSSH_IPC_SHIM__) return;
  observingHost = true;
  // JCEF injects its chooser after onLoadEnd, which can follow the first
  // capability response. Refresh only capabilities; mounted sessions stay live.
  window.addEventListener("rssh:host-ready", () => {
    // A failed first load belongs to App's retry flow, which also starts the
    // remaining application services. Host readiness must not bypass it.
    const initialized = pending ?? (_loaded ? Promise.resolve() : undefined);
    if (!initialized) return;
    void initialized.then(requestCapabilities)
      .catch((error) => console.warn("[runtime] host capability refresh failed:", error));
  });
}
