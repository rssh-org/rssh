import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Subscribe before enabling the native callback; serialize updates and teardown. */
export function connectSystemBack(windowLabel: string, onBack: () => void, onError: (error: unknown) => void) {
  let enabled = false;
  let disposed = false;
  let applied: boolean | undefined;
  let unlisten: UnlistenFn | undefined;
  let pending = listen(`navigation:back:${windowLabel}`, () => {
    if (!disposed) onBack();
  }).then((stop) => { unlisten = stop; }).catch(onError);

  function sync() {
    pending = pending.then(async () => {
      if (!unlisten) return;
      const next = !disposed && enabled;
      if (applied === next) return;
      await invoke("set_back_navigation_enabled", { enabled: next });
      applied = next;
    }).catch(onError);
  }

  sync();
  return {
    setEnabled(value: boolean) {
      if (disposed) return;
      enabled = value;
      sync();
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      sync();
      void pending.then(() => { unlisten?.(); unlisten = undefined; });
    },
  };
}
