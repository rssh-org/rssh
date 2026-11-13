interface HomeRefreshDependencies<StaticData> {
  loadStatic: () => Promise<StaticData>;
  applyStatic: (data: StaticData) => void;
  onError?: (error: unknown) => void;
}

export function createHomeRefresh<StaticData>(
  dependencies: HomeRefreshDependencies<StaticData>,
) {
  let generation = 0;

  return {
    async refresh(): Promise<void> {
      const current = ++generation;
      try {
        const staticData = await dependencies.loadStatic();
        if (current !== generation) return;
        dependencies.applyStatic(staticData);

      } catch (error) {
        if (current === generation) dependencies.onError?.(error);
      }
    },
    cancel(): void {
      generation += 1;
    },
  };
}
