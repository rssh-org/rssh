interface PrimarySessionWindowDependencies {
  signal?: AbortSignal;
  canOpenLocal?: boolean;
  reconcile: () => Promise<unknown>;
  allowResourcePanes: () => void;
  restoreSessions?: () => Promise<boolean>;
  loadAutoOpenLocal: () => Promise<boolean>;
  openLocal: () => void;
}

export async function initializePrimarySessionWindow(
  dependencies: PrimarySessionWindowDependencies,
): Promise<void> {
  if (dependencies.signal?.aborted) return;
  await dependencies.reconcile();
  if (dependencies.signal?.aborted) return;
  dependencies.allowResourcePanes();
  let restoredLocal = false;
  try {
    restoredLocal = await dependencies.restoreSessions?.() ?? false;
  } catch {
    // A failed restore must not prevent the existing startup preference.
  }
  if (dependencies.signal?.aborted || restoredLocal) return;
  if (dependencies.canOpenLocal === false) return;

  try {
    const autoOpen = await dependencies.loadAutoOpenLocal();
    if (!dependencies.signal?.aborted && autoOpen) {
      dependencies.openLocal();
    }
  } catch {
    // Keep the existing fail-closed auto-open behavior when settings are
    // unavailable; the resource barrier has already been released above.
  }
}
