// Keep the text clipped at rest; only move overflowing lines under the mouse.
export function homeHoverScroll(node: HTMLElement, _content: readonly [string, string]) {
  const lines = Array.from(node.querySelectorAll<HTMLElement>(".card-text"));
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  let hovered = false;
  let animations: Animation[] = [];

  function reset() {
    for (const animation of animations) animation.cancel();
    animations = [];
    for (const line of lines) line.style.removeProperty("overflow");
  }

  function refresh() {
    reset();
    if (!hovered || reducedMotion.matches) return;

    for (const line of lines) {
      const distance = line.scrollWidth - line.clientWidth;
      if (line.clientWidth === 0 || distance <= 0) continue;

      const pause = 800;
      const travel = distance / 30 * 1000;
      const duration = 2 * (pause + travel);
      const end = `translateX(-${distance}px)`;
      // The outer line still clips the text, while the inner span can reveal
      // its full contents without changing the card's dimensions.
      line.style.overflow = "visible";
      animations.push(line.animate([
        { transform: "translateX(0)", offset: 0 },
        { transform: "translateX(0)", offset: pause / duration },
        { transform: end, offset: (pause + travel) / duration },
        { transform: end, offset: (2 * pause + travel) / duration },
        { transform: "translateX(0)", offset: 1 },
      ], { duration, iterations: Infinity, easing: "linear" }));
    }
  }

  const observer = new ResizeObserver(refresh);

  function enter(event: PointerEvent) {
    if (event.pointerType !== "mouse") return;
    hovered = true;
    // The first observer callback starts scrolling after layout is measured.
    for (const line of lines) observer.observe(line);
    reducedMotion.addEventListener("change", refresh);
  }

  function leave() {
    hovered = false;
    observer.disconnect();
    reducedMotion.removeEventListener("change", refresh);
    reset();
  }

  node.addEventListener("pointerenter", enter);
  node.addEventListener("pointerleave", leave);
  node.addEventListener("pointercancel", leave);

  return {
    update() {
      // Keyed cards can receive refreshed text without being remounted.
      queueMicrotask(refresh);
    },
    destroy() {
      leave();
      node.removeEventListener("pointerenter", enter);
      node.removeEventListener("pointerleave", leave);
      node.removeEventListener("pointercancel", leave);
    },
  };
}
