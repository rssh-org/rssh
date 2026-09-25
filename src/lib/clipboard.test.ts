import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock, host } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  host: { nativeClipboard: true },
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("./stores/runtime.svelte.ts", () => ({ capabilities: () => host }));

beforeEach(() => { host.nativeClipboard = true; });
afterEach(() => {
  invokeMock.mockReset();
  vi.unstubAllGlobals();
});

describe("text clipboard", () => {
  it.each(["Macintosh", "Android 15", "iPhone", "OpenHarmony 5.0"])(
    "uses the native adapter when the host provides it, regardless of %s", async (userAgent) => {
      const browserRead = vi.fn();
      const browserWrite = vi.fn();
      vi.stubGlobal("navigator", { userAgent, clipboard: { readText: browserRead, writeText: browserWrite } });
      invokeMock.mockImplementation(async (command) => command === "clipboard_read" ? "clipboard text" : undefined);
      const clipboard = await import("./clipboard.ts");
      await expect(clipboard.readText()).resolves.toBe("clipboard text");
      await clipboard.writeText("hello");
      expect(invokeMock).toHaveBeenCalledWith("clipboard_read");
      expect(invokeMock).toHaveBeenCalledWith("clipboard_write", { text: "hello" });
      expect(browserRead).not.toHaveBeenCalled();
      expect(browserWrite).not.toHaveBeenCalled();
    },
  );

  it("preserves the WebView/browser clipboard path on hosts without a native adapter", async () => {
    host.nativeClipboard = false;
    const readText = vi.fn().mockResolvedValue("browser text");
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { userAgent: "Macintosh", clipboard: { readText, writeText } });
    const clipboard = await import("./clipboard.ts");
    await expect(clipboard.readText()).resolves.toBe("browser text");
    await clipboard.writeText("hello");
    expect(writeText).toHaveBeenCalledWith("hello");
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it.each([true, false])("preserves failures for nativeClipboard=%s", async (nativeClipboard) => {
    host.nativeClipboard = nativeClipboard;
    const failure = new Error("clipboard unavailable");
    invokeMock.mockRejectedValue(failure);
    vi.stubGlobal("navigator", { clipboard: {
      readText: vi.fn().mockRejectedValue(failure),
      writeText: vi.fn().mockRejectedValue(failure),
    } });
    const clipboard = await import("./clipboard.ts");
    await expect(clipboard.readText()).rejects.toThrow("clipboard unavailable");
    await expect(clipboard.writeText("hello")).rejects.toThrow("clipboard unavailable");
  });
});
