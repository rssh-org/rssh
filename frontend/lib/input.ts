/** Input hardware is independent of both OS and window width. */
export function supportsTouch(): boolean {
  return (typeof navigator !== "undefined" && navigator.maxTouchPoints > 0)
    || (typeof window !== "undefined" && window.matchMedia?.("(any-pointer: coarse)").matches === true);
}

let lastPointerType = "";

/** Older WebViews deliver contextmenu as MouseEvent without pointerType. */
export function installInputTracking(target: Document = document): () => void {
  const onPointerDown = (event: PointerEvent) => { lastPointerType = event.pointerType; };
  const onKeyDown = () => { lastPointerType = ""; };
  const options = { capture: true };
  target.addEventListener("pointerdown", onPointerDown, options);
  target.addEventListener("keydown", onKeyDown, options);
  return () => {
    target.removeEventListener("pointerdown", onPointerDown, options);
    target.removeEventListener("keydown", onKeyDown, options);
    lastPointerType = "";
  };
}

export function isTouchContextMenu(event: MouseEvent): boolean {
  // Chromium may expose a touch-generated compatibility event as a mouse
  // event. Prefer its explicit touch source without blocking a real mouse.
  const sourceCapabilities = (event as MouseEvent & {
    sourceCapabilities?: { firesTouchEvents: boolean } | null;
  }).sourceCapabilities;
  if (sourceCapabilities?.firesTouchEvents === true) return true;
  const pointerType = (event as PointerEvent).pointerType || lastPointerType;
  return pointerType === "touch" || pointerType === "pen";
}
