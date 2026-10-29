import { getContext, setContext, untrack } from "svelte";

export interface BackHandler {
  handle: () => void;
  enabled?: () => boolean;
  /** Page navigation sits below transient UI; peers dismiss newest first. */
  priority?: number;
}

let handlers = $state.raw<BackHandler[]>([]);
const backScope = Symbol("back-scope");

/** Keep-alive panes expose visibility without destroying their UI state. */
export function setBackScope(active: () => boolean): void {
  const parent = getContext<(() => boolean) | undefined>(backScope);
  setContext(backScope, () => (!parent || parent()) && active());
}

/** Capture the owning pane during component initialization, before effects run. */
export function createScopedBackHandler(): typeof registerBackHandler {
  const active = getContext<(() => boolean) | undefined>(backScope);
  return handler => registerBackHandler({
    ...handler,
    enabled: () => (!active || active()) && (!handler.enabled || handler.enabled()),
  });
}

export function registerBackHandler(handler: BackHandler): () => void {
  handlers = [...untrack(() => handlers), handler];
  return () => { handlers = untrack(() => handlers).filter((entry) => entry !== handler); };
}

function activeHandler(): BackHandler | undefined {
  let active: BackHandler | undefined;
  for (const handler of handlers) {
    if (handler.enabled && !handler.enabled()) continue;
    if (!active || (handler.priority ?? 1) >= (active.priority ?? 1)) active = handler;
  }
  return active;
}

export function canHandleBack(): boolean { return activeHandler() !== undefined; }

export function requestBack(): boolean {
  const handler = activeHandler();
  if (!handler) return false;
  handler.handle();
  return true;
}
