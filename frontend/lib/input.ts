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
  // Native selection-handle drags have no DOM pointerdown. Older Android
  // WebViews label their contextmenu as "mouse", even after touch selection:
  // https://issues.chromium.org/issues/41487195
  // Keep the last actual touch/pen input until a mouse pointerdown or keydown
  // replaces it; a contextmenu event alone must not switch the input source.
  const pointerType = (event as PointerEvent).pointerType;
  return pointerType === "touch" || pointerType === "pen"
    || lastPointerType === "touch" || lastPointerType === "pen";
}
