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
  target.addEventListener("pointerdown", onPointerDown, true);
  target.addEventListener("keydown", onKeyDown, true);
  return () => {
    target.removeEventListener("pointerdown", onPointerDown, true);
    target.removeEventListener("keydown", onKeyDown, true);
    lastPointerType = "";
  };
}

export function isTouchContextMenu(event: MouseEvent): boolean {
  const pointerType = (event as PointerEvent).pointerType || lastPointerType;
  return pointerType === "touch" || pointerType === "pen";
}
