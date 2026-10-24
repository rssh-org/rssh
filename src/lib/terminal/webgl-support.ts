import { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";
import { disposeWebglAddon } from "./webgl-addon";

/** Check initialization and early context loss; this does not measure performance. */
export async function assertWebglAvailable(): Promise<void> {
  const host = document.createElement("div");
  // Keep layout measurable for xterm's DOM font-metrics fallback.
  host.style.cssText = "position:fixed;left:-10000px;top:0;width:80px;height:40px;pointer-events:none";
  host.inert = true;
  host.setAttribute("aria-hidden", "true");

  let terminal: Terminal | undefined;
  let addon: WebglAddon | undefined;
  let onContextLoss: EventListener | undefined;
  let frame: number | undefined;
  let timeout: number | undefined;
  try {
    await new Promise<void>((resolve, reject) => {
      // Capture the native event: the addon's notification waits three seconds.
      onContextLoss = () => reject(new Error("WebGL context lost during initialization"));
      host.addEventListener("webglcontextlost", onContextLoss, true);
      document.body.appendChild(host);
      terminal = new Terminal({ cols: 2, rows: 1, scrollback: 0, cursorBlink: false, disableStdin: true });
      // Opening first makes addon activation synchronous, so failures propagate.
      terminal.open(host);
      addon = new WebglAddon();
      terminal.loadAddon(addon);
      if (!addon.textureAtlas) throw new Error("WebGL texture atlas did not initialize");
      // Allow startup events to arrive without waiting indefinitely in background windows.
      timeout = window.setTimeout(resolve, 100);
      frame = window.requestAnimationFrame(() => {
        frame = window.requestAnimationFrame(() => resolve());
      });
    });
    // The native loss event can arrive after the context has already been lost.
    // The other attached canvas is xterm's 2D link layer.
    const canvas = host.querySelector<HTMLCanvasElement>(".xterm-screen > canvas:not(.xterm-link-layer)");
    const context = canvas?.getContext("webgl2");
    if (!context) throw new Error("WebGL canvas did not initialize");
    if (context.isContextLost()) throw new Error("WebGL context lost during initialization");
  } finally {
    if (onContextLoss) host.removeEventListener("webglcontextlost", onContextLoss, true);
    if (frame !== undefined) window.cancelAnimationFrame(frame);
    if (timeout !== undefined) window.clearTimeout(timeout);
    try {
      disposeWebglAddon(addon);
    } finally {
      try {
        terminal?.dispose();
      } finally {
        host.remove();
      }
    }
  }
}
