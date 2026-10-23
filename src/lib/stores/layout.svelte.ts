/** Layout follows the CSS viewport, independently of the host and input devices. */
const COMPACT_BREAKPOINT = 640;
function readViewportWidth(): number {
  return typeof window !== "undefined" && Number.isFinite(window.innerWidth)
    ? window.innerWidth
    : 1024;
}
let _viewportWidth = $state(readViewportWidth());

export function viewportWidth(): number { return _viewportWidth; }
export function compact(): boolean { return _viewportWidth < COMPACT_BREAKPOINT; }

/** The application root owns this listener; resizing never changes preferences. */
export function observeViewport(): () => void {
  const update = () => { _viewportWidth = readViewportWidth(); };
  update();
  window.addEventListener("resize", update);
  return () => window.removeEventListener("resize", update);
}
