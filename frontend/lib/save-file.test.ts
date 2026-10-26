import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

let win: any;
let anchor: any;
let saveTextFile: typeof import("./save-file.ts").saveTextFile;

beforeEach(async () => {
    invokeMock.mockReset();
    vi.resetModules();
    vi.stubGlobal("navigator", { userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15)", maxTouchPoints: 0 });
    win = {};
    anchor = { href: "", download: "", style: {} as Record<string, string>, click: vi.fn(), remove: vi.fn() };
    vi.stubGlobal("window", win);
    vi.stubGlobal("document", { createElement: () => anchor, body: { appendChild() {} } });
    vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:x");
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
    ({ saveTextFile } = await import("./save-file.ts"));
});

afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
});

describe("saveTextFile", () => {
    it("browser: triggers a Blob download and returns the default name", async () => {
        const r = await saveTextFile("hello", { defaultName: "cfg.json" });
        expect(anchor.download).toBe("cfg.json");
        expect(anchor.click).toHaveBeenCalled();
        expect(r).toBe("cfg.json");
    });

    it("JCEF plugin host: rejects with a localizable error (downloads are dropped there)", async () => {
        win.__RSSH_PICK__ = vi.fn();
        await expect(saveTextFile("x", { defaultName: "cfg.json" })).rejects.toMatch(
            /file_save_unsupported_in_plugin/,
        );
    });

    it.each([
        "Mozilla/5.0 (Macintosh)",
        "Mozilla/5.0 (Linux; Android 14)",
        "Mozilla/5.0 (iPhone)",
        "Mozilla/5.0 (Phone; OpenHarmony 5.0)",
    ])("native runtime uses the same save command: %s", async (userAgent) => {
        win.__TAURI_INTERNALS__ = {};
        vi.stubGlobal("navigator", { userAgent });
        vi.resetModules();
        ({ saveTextFile } = await import("./save-file.ts"));
        invokeMock.mockResolvedValue("file://docs/cfg.json");
        const filters = [{ name: "JSON", extensions: ["json"] }];

        await expect(saveTextFile("data", { defaultName: "cfg.json", filters }))
            .resolves.toBe("file://docs/cfg.json");
        expect(invokeMock).toHaveBeenCalledExactlyOnceWith("save_text_file", {
            defaultName: "cfg.json", contents: "data", filters,
        });
    });

    it("native runtime preserves picker cancellation and write errors", async () => {
        win.__TAURI_INTERNALS__ = {};
        invokeMock.mockResolvedValueOnce(null);
        await expect(saveTextFile("data", { defaultName: "cfg.json" })).resolves.toBeNull();
        invokeMock.mockRejectedValueOnce(new Error("native write failed"));
        await expect(saveTextFile("data", { defaultName: "cfg.json" })).rejects.toThrow("native write failed");
    });

    it("HarmonyOS browser with the IPC shim still uses a Blob download", async () => {
        win.__TAURI_INTERNALS__ = {};
        win.__RSSH_IPC_SHIM__ = true;
        vi.stubGlobal("navigator", { userAgent: "Mozilla/5.0 (Phone; OpenHarmony 5.0)" });
        vi.resetModules();
        ({ saveTextFile } = await import("./save-file.ts"));

        await expect(saveTextFile("data", { defaultName: "cfg.json" })).resolves.toBe("cfg.json");
        expect(anchor.click).toHaveBeenCalled();
        expect(invokeMock).not.toHaveBeenCalled();
    });

    it("JCEF with the IPC shim preserves its explicit unsupported-save error", async () => {
        win.__TAURI_INTERNALS__ = {};
        win.__RSSH_IPC_SHIM__ = true;
        win.__RSSH_PICK__ = vi.fn();

        await expect(saveTextFile("data", { defaultName: "cfg.json" })).rejects.toMatch(/file_save_unsupported_in_plugin/);
        expect(anchor.click).not.toHaveBeenCalled();
        expect(invokeMock).not.toHaveBeenCalled();
    });
});
