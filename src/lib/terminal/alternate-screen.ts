import type { OutputFeeder } from "./output-feeder.ts";

/**
 * Per-tab bridge to xterm's live alternate-screen state. AI commands write
 * directly to the transport, so they must check whether a fullscreen program
 * owns the terminal before pasting shell input. Keep this module independent
 * of the app store, which already imports the AI store.
 */
type Provider = { read: () => boolean; output?: Pick<OutputFeeder, "flush" | "pendingBytes"> };
const providers = new Map<string, Provider>();

export function registerAlternateScreenProvider(
  tabId: string,
  read: () => boolean,
  output?: Provider["output"],
): void {
  providers.set(tabId, { read, output });
}

export function unregisterAlternateScreenProvider(tabId: string): void {
  providers.delete(tabId);
}

/** Read at the point of use: a pending approval can outlive a buffer switch. */
export function alternateScreenActive(tabId: string): boolean {
  return providers.get(tabId)?.read() ?? false;
}

/** A buffer mode describes parsed output. Wait for bytes already received by
 * the pane before using it to authorize a transport write. A replaced/closed
 * pane cannot authorize the old caller when its pending flush completes. */
export function flushTerminalOutput(
  tabId: string,
  stillNeeded: () => boolean = () => true,
): Promise<boolean> | undefined {
  const provider = providers.get(tabId);
  if (!provider?.output) return;
  const output = provider.output;
  return (async () => {
    do {
      if (!await output.flush() || providers.get(tabId) !== provider || !stillNeeded()) return false;
      // More output may arrive after the parser goes idle, before this await
      // resumes. Do not authorize while those newer bytes are still queued.
    } while (output.pendingBytes() > 0);
    return true;
  })();
}
