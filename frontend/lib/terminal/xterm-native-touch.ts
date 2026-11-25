import type { Terminal } from "@xterm/xterm";
import { supportsTouch } from "../input.ts";

/** Expose xterm's input at the cursor for WebKit's native Paste menu.
 * Real mouse input and terminal applications' mouse reporting keep xterm's path. */
export function setupXtermNativeTouch({ terminal, host, enabled }: {
  terminal: Terminal;
  host: HTMLElement;
  enabled: boolean;
}): () => void {
  const helper = terminal.textarea;
  if (!enabled || !helper) return () => {};

  let touchInput = supportsTouch();

  function update() {
    const native = touchInput && terminal.modes.mouseTrackingMode === "none"
      && !!host.querySelector(".xterm-rows");
    host.classList.toggle("xterm-native-touch", native);
    const buffer = terminal.buffer.active;
    const screen = host.querySelector<HTMLElement>(".xterm-screen");
    const cursor = native && buffer.viewportY === buffer.baseY
      && !!host.querySelector(".xterm-cursor") && !!screen
      && terminal.cols > 0 && terminal.rows > 0;
    host.classList.toggle("xterm-native-cursor", cursor);
    if (cursor && screen) {
      // xterm does not reposition its helper when returning from scrollback.
      const bounds = screen.getBoundingClientRect();
      host.style.setProperty("--native-cursor-left", `${Math.min(buffer.cursorX, terminal.cols - 1) * bounds.width / terminal.cols}px`);
      host.style.setProperty("--native-cursor-top", `${buffer.cursorY * bounds.height / terminal.rows}px`);
    }
  }

  function onPointer(event: PointerEvent) {
    if (event.pointerType === "mouse") touchInput = false;
    else if (event.pointerType === "touch" || event.pointerType === "pen") touchInput = true;
    update();
  }

  function onPaste(event: ClipboardEvent) {
    // xterm still owns paste -> onData, including bracketed paste. Suppress only
    // the browser's second insertion into the helper, which xterm already clears.
    if (event.target === helper && event.clipboardData) event.preventDefault();
  }

  function onCopy(event: ClipboardEvent) {
    const selection = host.ownerDocument.getSelection();
    // xterm's previous mouse selection must not overwrite the native selection.
    // Leave the default Copy action to WebKit without rebuilding either range.
    if (selection && !selection.isCollapsed
      && host.contains(selection.anchorNode) && host.contains(selection.focusNode)) {
      event.stopPropagation();
    }
  }

  host.addEventListener("pointerover", onPointer, true);
  host.addEventListener("pointerdown", onPointer, true);
  host.addEventListener("paste", onPaste, true);
  host.addEventListener("copy", onCopy, true);
  const subscriptions = [terminal.onRender(update), terminal.onScroll(update), terminal.onCursorMove(update)];
  update();
  return () => {
    host.removeEventListener("pointerover", onPointer, true);
    host.removeEventListener("pointerdown", onPointer, true);
    host.removeEventListener("paste", onPaste, true);
    host.removeEventListener("copy", onCopy, true);
    for (const subscription of subscriptions) subscription.dispose();
    host.classList.remove("xterm-native-touch", "xterm-native-cursor");
    host.style.removeProperty("--native-cursor-left");
    host.style.removeProperty("--native-cursor-top");
  };
}
