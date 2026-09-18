import { afterEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

afterEach(() => {
  invokeMock.mockReset();
  vi.unstubAllGlobals();
});

describe("text clipboard", () => {
  it.each(["Macintosh", "Android 15", "iPhone", "OpenHarmony 5.0"])(
    "uses the same native commands on %s", async (userAgent) => {
      const browserRead = vi.fn();
      const browserWrite = vi.fn();
      vi.stubGlobal("navigator", { userAgent, clipboard: { readText: browserRead, writeText: browserWrite } });
      invokeMock.mockImplementation(async (command) => command === "clipboard_read" ? "clipboard text" : undefined);
      vi.resetModules();
      const clipboard = await import("./clipboard.ts");

      await expect(clipboard.readText()).resolves.toBe("clipboard text");
      await expect(clipboard.writeText("hello")).resolves.toBeUndefined();
      expect(invokeMock).toHaveBeenCalledWith("clipboard_read");
      expect(invokeMock).toHaveBeenCalledWith("clipboard_write", { text: "hello" });
      expect(browserRead).not.toHaveBeenCalled();
      expect(browserWrite).not.toHaveBeenCalled();
    },
  );

  it("preserves failures from the host adapter", async () => {
    invokeMock.mockRejectedValue(new Error("clipboard unavailable"));
    const clipboard = await import("./clipboard.ts");
    await expect(clipboard.readText()).rejects.toThrow("clipboard unavailable");
    await expect(clipboard.writeText("hello")).rejects.toThrow("clipboard unavailable");
  });
});
