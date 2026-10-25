/**
 * Per-tab bridge to xterm's live alternate-screen state. AI commands write
 * directly to the transport, so they must check whether a fullscreen program
 * owns the terminal before pasting shell input. Keep this module independent
 * of the app store, which already imports the AI store.
 */
const providers = new Map<string, () => boolean>();

export function registerAlternateScreenProvider(tabId: string, read: () => boolean): void {
  providers.set(tabId, read);
}

export function unregisterAlternateScreenProvider(tabId: string): void {
  providers.delete(tabId);
}

/** Read at the point of use: a pending approval can outlive a buffer switch. */
export function alternateScreenActive(tabId: string): boolean {
  return providers.get(tabId)?.() ?? false;
}
