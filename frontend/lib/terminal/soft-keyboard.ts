import { setupSoftKeyboardInset } from "../soft-keyboard-inset.ts";

interface SoftKeyboardOptions {
  helper: HTMLTextAreaElement;
  host: HTMLElement;
  pane: HTMLElement;
  touchAvailable: boolean;
  onRequestedChange: (requested: boolean) => void;
}

/** Explicit soft-keyboard requests are separate from terminal focus. A locked
 * textarea stays focusable so a hardware keyboard can unlock it on its first key. */
export function setupTerminalSoftKeyboard({
  helper, host, pane, touchAvailable, onRequestedChange,
}: SoftKeyboardOptions) {
  let unpinnedStyle: string | null | undefined;
  const originalInputMode = helper.getAttribute("inputmode");
  const originalReadOnly = helper.readOnly;
  let touchInput = touchAvailable;
  let requested = false;
  let scrollResetRaf = 0;
  let helperPinRaf = 0;

  function setRequested(value: boolean) {
    if (requested === value) return;
    requested = value;
    onRequestedChange(value);
  }

  function restoreHelperStyle() {
    if (helperPinRaf) cancelAnimationFrame(helperPinRaf);
    helperPinRaf = 0;
    if (unpinnedStyle === undefined) return;
    if (unpinnedStyle === null) helper.removeAttribute("style");
    else helper.setAttribute("style", unpinnedStyle);
    unpinnedStyle = undefined;
  }

  function resetDocumentScroll() {
    if (scrollResetRaf) return;
    scrollResetRaf = requestAnimationFrame(() => {
      scrollResetRaf = 0;
      if (!requested) return;
      if (window.scrollX !== 0 || window.scrollY !== 0) window.scrollTo(0, 0);
      document.documentElement.scrollTop = document.body.scrollTop = 0;
      document.documentElement.scrollLeft = document.body.scrollLeft = 0;
    });
  }

  function pinHelper() {
    if (!requested) return;
    if (unpinnedStyle === undefined) unpinnedStyle = helper.getAttribute("style");
    const viewport = window.visualViewport;
    const minTop = (viewport?.offsetTop ?? 0) + 1;
    const maxTop = minTop + (viewport?.height ?? window.innerHeight) - 2;
    const top = Math.max(minTop, Math.min(maxTop, host.getBoundingClientRect().top + 8));
    // Pin only while explicitly requesting the soft keyboard. Ordinary mouse
    // and hardware-IME input keep xterm's own caret-aligned helper geometry.
    Object.assign(helper.style, {
      position: "fixed", left: "1px", top: `${Math.round(top)}px`,
      width: "1px", height: "1px", opacity: "0", zIndex: "-1",
      pointerEvents: "none", caretColor: "transparent", background: "transparent",
      color: "transparent", border: "0", padding: "0", margin: "0",
      outline: "0", resize: "none", overflow: "hidden",
    });
  }

  function keepHelperInView() {
    if (!requested) return;
    pinHelper();
    resetDocumentScroll();
    if (helperPinRaf) cancelAnimationFrame(helperPinRaf);
    helperPinRaf = requestAnimationFrame(() => {
      helperPinRaf = 0;
      pinHelper();
      resetDocumentScroll();
    });
  }

  function lock() {
    helper.readOnly = true;
    helper.setAttribute("inputmode", "none");
  }

  function unlock() {
    helper.readOnly = false;
    helper.setAttribute("inputmode", "text");
  }

  function hide() {
    setRequested(false);
    helper.blur();
    if (touchInput) lock();
    restoreHelperStyle();
  }

  function show() {
    // Re-focus after readonly so an already-focused helper can summon the IME.
    helper.blur();
    setRequested(true);
    unlock();
    pinHelper();
    helper.focus({ preventScroll: true });
    resetDocumentScroll();
  }

  function onPointerDown(event: PointerEvent) {
    if (event.pointerType === "touch" || event.pointerType === "pen") {
      touchInput = true;
      hide();
    } else if (event.pointerType === "mouse") {
      touchInput = false;
      setRequested(false);
      restoreHelperStyle();
      unlock();
    }
  }

  function onHardwareInput(event: Event) {
    if (event.target !== helper) return;
    if (helper.readOnly) {
      unlock();
      restoreHelperStyle();
    }
    keepHelperInView();
  }

  function onBlur() {
    setRequested(false);
    if (touchInput) lock();
    restoreHelperStyle();
  }

  function onViewportChange() {
    if (requested && document.activeElement === helper) keepHelperInView();
  }

  if (touchInput) lock();
  host.addEventListener("pointerdown", onPointerDown, { capture: true, passive: true });
  // xterm already registered capture listeners on the textarea. Capture at its
  // ancestor runs first regardless of registration order. Never preventDefault:
  // the first hardware key (including an IME starter) must reach xterm.
  host.addEventListener("keydown", onHardwareInput, true);
  host.addEventListener("compositionstart", onHardwareInput, true);
  helper.addEventListener("input", keepHelperInView);
  helper.addEventListener("compositionupdate", keepHelperInView);
  helper.addEventListener("blur", onBlur);
  window.addEventListener("scroll", onViewportChange, { passive: true });
  window.visualViewport?.addEventListener("scroll", onViewportChange, { passive: true });
  window.visualViewport?.addEventListener("resize", onViewportChange, { passive: true });
  const insetCleanup = setupSoftKeyboardInset(pane, helper);

  return {
    toggle: () => { if (requested) hide(); else show(); },
    hide,
    dispose() {
      host.removeEventListener("pointerdown", onPointerDown, true);
      host.removeEventListener("keydown", onHardwareInput, true);
      host.removeEventListener("compositionstart", onHardwareInput, true);
      helper.removeEventListener("input", keepHelperInView);
      helper.removeEventListener("compositionupdate", keepHelperInView);
      helper.removeEventListener("blur", onBlur);
      window.removeEventListener("scroll", onViewportChange);
      window.visualViewport?.removeEventListener("scroll", onViewportChange);
      window.visualViewport?.removeEventListener("resize", onViewportChange);
      insetCleanup();
      setRequested(false);
      restoreHelperStyle();
      if (scrollResetRaf) cancelAnimationFrame(scrollResetRaf);
      helper.readOnly = originalReadOnly;
      if (originalInputMode === null) helper.removeAttribute("inputmode");
      else helper.setAttribute("inputmode", originalInputMode);
    },
  };
}
