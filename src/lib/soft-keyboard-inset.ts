/**
 * Soft-keyboard inset for bottom-docked panels (terminal pane, AI chat panel).
 *
 * WKWebView does NOT resize the webview when the soft keyboard opens — the
 * keyboard overlays it. The layout viewport (the `100%` heights everything is
 * built on) stays full-size while only `visualViewport` shrinks, so content
 * laid out at the bottom (the MobileKeybar, the chat input row) ends up
 * behind the keyboard, untappable. Android needs no help: `adjustResize`
 * shrinks the webview itself and the layout follows.
 *
 * Fix: while the panel's input holds focus, pad the panel's bottom by the gap
 * between the panel's real bottom edge and the keyboard top (= the visual
 * viewport's bottom). Flex content above shrinks and the bottom row rides up
 * flush onto the keyboard. The same formula measures 0 on Android (visual
 * viewport already fills the window), so it collapses to a no-op there — one
 * code path, no platform fork.
 */

/** Viewport geometry we need; structurally matches window.visualViewport. */
export interface ViewportRect {
  offsetTop: number;
  height: number;
}

/**
 * Padding (px) that lifts the pane's content onto the visible viewport:
 * paneBottom - (offsetTop + height), clamped at 0 and rounded to whole px
 * (fractional values churn the style during the keyboard animation).
 */
export function softKeyboardInset(paneBottom: number, viewport: ViewportRect): number {
  return Math.max(0, Math.round(paneBottom - viewport.offsetTop - viewport.height));
}

/**
 * Apply (or clear) the inset on the panel root, returning the applied value —
 * callers use the return to detect "keyboard fully gone" (0). Panel position
 * is measured, not assumed, so whatever chrome sits below the pane (safe-area
 * padding, tab bars) is automatically accounted for. Valid while the document
 * is pinned to scroll 0 — which setupMobileSoftKeyboard already enforces —
 * because client coords and visualViewport offsets only agree there.
 */
export function applySoftKeyboardInset(pane: HTMLElement): number {
  const vv = window.visualViewport;
  const inset = softKeyboardInset(
    pane.getBoundingClientRect().bottom,
    vv ?? { offsetTop: 0, height: window.innerHeight },
  );
  if (inset > 0) pane.style.paddingBottom = `${inset}px`;
  else pane.style.removeProperty("padding-bottom");
  return inset;
}

/** Drop the inset — keyboard hidden or the panel going away. */
export function clearSoftKeyboardInset(pane: HTMLElement): void {
  pane.style.removeProperty("padding-bottom");
}

/**
 * Keep `panel` clear of the soft keyboard while `input` holds focus, riding
 * the keyboard up AND down (viewport events track the open/close animation
 * frames). Returns a cleanup fn. Listens on the input's focus/blur plus
 * visualViewport resize/scroll; no-op wherever the visual viewport already
 * fills the window (Android's adjustResize, desktop).
 *
 * Settle mode: blur fires while the keyboard is still sliding away, so
 * tracking continues (inset shrinks toward 0) instead of snapping the bottom
 * row back under the closing keyboard. Invariant: settling == true iff the
 * input is unfocused AND the panel still carries an inset.
 */
export function setupSoftKeyboardInset(panel: HTMLElement, input: HTMLElement): () => void {
  let settling = false;

  function onViewportChange() {
    if (document.activeElement === input) {
      applySoftKeyboardInset(panel);
    } else if (settling && applySoftKeyboardInset(panel) === 0) {
      settling = false;
    }
  }
  function onFocus() {
    settling = false;
    // Apply immediately: on a focus handoff (e.g. terminal keybar keyboard →
    // AI input) the keyboard is already open and stationary, so no viewport
    // event will fire — without this the newly focused panel would stay
    // unpadded behind the keyboard. When the keyboard is closed this
    // computes 0 and is a no-op; the open animation's viewport events take
    // over from there.
    applySoftKeyboardInset(panel);
  }
  function onBlur() {
    settling = applySoftKeyboardInset(panel) > 0;
  }

  input.addEventListener("focus", onFocus);
  input.addEventListener("blur", onBlur);
  const vv = window.visualViewport;
  vv?.addEventListener("resize", onViewportChange, { passive: true });
  vv?.addEventListener("scroll", onViewportChange, { passive: true });
  return () => {
    clearSoftKeyboardInset(panel);
    input.removeEventListener("focus", onFocus);
    input.removeEventListener("blur", onBlur);
    vv?.removeEventListener("resize", onViewportChange);
    vv?.removeEventListener("scroll", onViewportChange);
  };
}
