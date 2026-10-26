import type { WebglAddon } from "@xterm/addon-webgl";
import type { IDisposable } from "@xterm/xterm";

interface WebglAddonInternals {
  _renderer?: { _cursorBlinkStateManager?: IDisposable };
}

export function disposeWebglAddon(addon: WebglAddon | null | undefined): void {
  if (!addon) return;

  // @xterm/addon-webgl 0.19.0 omits this manager from renderer disposal,
  // leaving blink timers that retain the renderer and terminal. Remove this
  // private-API workaround once our dependency includes the upstream fix:
  // https://github.com/xtermjs/xterm.js/pull/5817
  try {
    (addon as WebglAddon & WebglAddonInternals)._renderer?._cursorBlinkStateManager?.dispose();
  } catch (error) {
    console.warn("[terminal] WebGL cursor blink cleanup failed:", error);
  }
  addon.dispose();
}
