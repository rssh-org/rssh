interface PrimarySessionWindowDependencies {
  signal?: AbortSignal;
  reconcile: () => Promise<unknown>;
  allowResourcePanes: () => void;
  runStartupAction: () => Promise<void>;
}

export async function initializePrimarySessionWindow(
  dependencies: PrimarySessionWindowDependencies,
): Promise<void> {
  if (dependencies.signal?.aborted) return;
  await dependencies.reconcile();
  if (dependencies.signal?.aborted) return;
  dependencies.allowResourcePanes();
  await dependencies.runStartupAction();
}
