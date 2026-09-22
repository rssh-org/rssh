import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installTauriShim, onRuntimeCapabilitiesChanged } from "./ipc-shim.ts";

// The shim targets the browser; vitest runs in `node`, so we stub the handful of
// globals it touches (window, location, WebSocket, navigator) with controllable
// fakes. A fake WebSocket lets us drive the open / message / close lifecycle and
// assert the exact wire frames the shim sends.

type WsMsg = { data: string };

class FakeWS {
    static instances: FakeWS[] = [];
    static OPEN = 1;
    static last(): FakeWS {
        return FakeWS.instances[FakeWS.instances.length - 1];
    }
    url: string;
    readyState = 0;
    sent: string[] = [];
    onopen: (() => void) | null = null;
    onmessage: ((e: WsMsg) => void) | null = null;
    onclose: (() => void) | null = null;
    constructor(url: string) {
        this.url = url;
        FakeWS.instances.push(this);
    }
    send(s: string) {
        this.sent.push(s);
    }
    close() {}
    // --- test drivers ---
    open() {
        this.readyState = FakeWS.OPEN;
        this.onopen?.();
    }
    deliver(obj: unknown) {
        this.onmessage?.({ data: JSON.stringify(obj) });
    }
    sentFrames(): any[] {
        return this.sent.map((s) => JSON.parse(s));
    }
}

let fakeWindow: any;
let clipboardText = "";

beforeEach(() => {
    FakeWS.instances = [];
    clipboardText = "from-os-clipboard";
    fakeWindow = new EventTarget();
    vi.stubGlobal("window", fakeWindow);
    vi.stubGlobal("location", { search: "", pathname: "/", hash: "" });
    vi.stubGlobal("WebSocket", FakeWS as unknown as typeof WebSocket);
    vi.stubGlobal("navigator", {
        clipboard: {
            readText: vi.fn(async () => clipboardText),
            writeText: vi.fn(async (t: string) => {
                clipboardText = t;
            }),
        },
    });
    vi.stubGlobal("console", { ...console, warn: vi.fn() });
});

afterEach(() => {
    vi.unstubAllGlobals();
});

/** Install the shim with server coordinates and return the live internals + socket. */
function installWithServer() {
    fakeWindow.__RSSH_SERVER__ = { port: 5555, token: "tok-123" };
    installTauriShim();
    return {
        internals: fakeWindow.__TAURI_INTERNALS__ as {
            invoke: (cmd: string, args?: any) => Promise<unknown>;
            transformCallback: (cb: (p: unknown) => void, once?: boolean) => number;
        },
        ws: FakeWS.last(),
    };
}

describe("installTauriShim", () => {
    it("installs __TAURI_INTERNALS__ and dials the token-scoped loopback ws", () => {
        const { internals, ws } = installWithServer();
        expect(typeof internals.invoke).toBe("function");
        expect(typeof internals.transformCallback).toBe("function");
        expect(fakeWindow.__RSSH_IPC_SHIM__).toBe(true);
        expect(ws.url).toBe("ws://127.0.0.1:5555/?token=tok-123");
    });

    it("is a no-op when Tauri already injected the global (desktop, INV-2)", () => {
        const native = { invoke: vi.fn() };
        fakeWindow.__TAURI_INTERNALS__ = native;
        fakeWindow.__RSSH_SERVER__ = { port: 5555, token: "tok" };
        installTauriShim();
        expect(fakeWindow.__TAURI_INTERNALS__).toBe(native); // untouched
        expect(fakeWindow.__RSSH_IPC_SHIM__).toBeUndefined();
        expect(FakeWS.instances).toHaveLength(0); // never dialed
    });

    it("does not install when no server coordinates are present", () => {
        installTauriShim(); // no __RSSH_SERVER__, no URL params
        expect(fakeWindow.__TAURI_INTERNALS__).toBeUndefined();
        expect(fakeWindow.__RSSH_IPC_SHIM__).toBeUndefined();
        expect(FakeWS.instances).toHaveLength(0);
    });
});

describe("window/app plugin compatibility (embedded, off-Tauri)", () => {
    it("reports late host capability changes through the adapter subscription", () => {
        installWithServer();
        const changed = vi.fn();
        const stop = onRuntimeCapabilitiesChanged(changed);
        fakeWindow.dispatchEvent(new Event("rssh:host-ready"));
        expect(changed).toHaveBeenCalledTimes(1);
        stop();
        fakeWindow.dispatchEvent(new Event("rssh:host-ready"));
        expect(changed).toHaveBeenCalledTimes(1);
    });

    it("ignores IDE readiness events in a native host", () => {
        const changed = vi.fn();
        const stop = onRuntimeCapabilitiesChanged(changed);
        fakeWindow.dispatchEvent(new Event("rssh:host-ready"));
        expect(changed).not.toHaveBeenCalled();
        stop();
    });

    it.each([
        [false, true, false],
        [true, true, true],
        [true, false, false],
    ])("intersects file capabilities with the host picker (%s, %s)", async (hasPicker, serverSupport, expected) => {
        if (hasPicker) fakeWindow.__RSSH_PICK__ = vi.fn();
        const { internals, ws } = installWithServer();
        ws.open();
        const result = internals.invoke("get_runtime_capabilities");
        const request = ws.sentFrames()[0];
        expect(request.cmd).toBe("get_runtime_capabilities");
        const terminalPolicy = {
            imageStorageLimitMb: 128,
            imagePixelLimit: 16_000_000,
            outputBacklogLimitBytes: 128 * 1024 * 1024,
            gpuRenderDefault: true,
        };
        ws.deliver({ type: "response", id: request.id, ok: true, result: {
            localPty: true,
            fileMultiSelect: serverSupport,
            directoryTransfer: serverSupport,
            nativeClipboard: true,
            terminalPolicy,
        } });
        await expect(result).resolves.toEqual({
            localPty: true,
            fileMultiSelect: expected,
            directoryTransfer: expected,
            nativeClipboard: false,
            terminalPolicy,
        });
    });

    it("provides metadata so getCurrentWindow()/getCurrentWebview() don't throw", () => {
        const { internals } = installWithServer();
        // @tauri-apps/api reads these labels SYNCHRONOUSLY in the getters; a
        // missing `metadata` made getCurrentWindow() throw at mount (title effect,
        // pin, WelcomeScreen). One "main" window mirrors the single embedded webview.
        const meta = (internals as any).metadata;
        expect(meta?.currentWindow?.label).toBe("main");
        expect(meta?.currentWebview?.label).toBe("main");
    });

    it("answers window-plugin commands locally as no-ops (never over the ws)", async () => {
        const { internals, ws } = installWithServer();
        ws.open();
        await expect(internals.invoke("plugin:window|set_title", { value: "X" })).resolves.toBeUndefined();
        await expect(internals.invoke("plugin:window|set_always_on_top", { value: true })).resolves.toBeUndefined();
        await expect(internals.invoke("plugin:window|set_decorations", { value: false })).resolves.toBeUndefined();
        await expect(internals.invoke("plugin:window|is_decorated", {})).resolves.toBe(true);
        // No native window off-Tauri → none of these touch the socket.
        expect(ws.sentFrames().some((f) => String(f.cmd).startsWith("plugin:window|"))).toBe(false);
    });

    it("routes plugin:app|version over the ws (the server answers it, not the shim)", () => {
        const { internals, ws } = installWithServer();
        internals.invoke("plugin:app|version", {});
        ws.open();
        // getVersion() must reach the server's version arm — the shim doesn't fake it.
        expect(ws.sentFrames()[0]).toMatchObject({ type: "invoke", cmd: "plugin:app|version" });
    });
});

describe("invoke round-trip over ws", () => {
    it("queues before open, flushes on open, resolves on matching response", async () => {
        const { internals, ws } = installWithServer();
        const p = internals.invoke("list_profiles", { a: 1 });

        // Buffered until the socket opens (readyState 0).
        expect(ws.sent).toHaveLength(0);
        ws.open();
        const frame = ws.sentFrames()[0];
        expect(frame).toMatchObject({ type: "invoke", cmd: "list_profiles", args: { a: 1 } });

        ws.deliver({ type: "response", id: frame.id, ok: true, result: [{ id: "p1" }] });
        await expect(p).resolves.toEqual([{ id: "p1" }]);
    });

    it("rejects with the server error on ok:false", async () => {
        const { internals, ws } = installWithServer();
        const p = internals.invoke("ssh_connect", {});
        ws.open();
        const id = ws.sentFrames()[0].id;
        ws.deliver({ type: "response", id, ok: false, error: "boom" });
        await expect(p).rejects.toBe("boom");
    });

    it("fails in-flight invokes when the socket closes", async () => {
        const { internals, ws } = installWithServer();
        const p = internals.invoke("sftp_list", {});
        ws.open();
        ws.onclose?.();
        await expect(p).rejects.toThrow(/connection closed/);
    });

    it("rejects (does not hang) invokes issued after the socket has closed", async () => {
        const { internals, ws } = installWithServer();
        ws.open();
        ws.onclose?.();
        // No reconnect + no live socket: this must settle, not queue forever.
        await expect(internals.invoke("list_profiles", {})).rejects.toThrow(/connection closed/);
    });
});

describe("event protocol (listen / push / unlisten)", () => {
    it("delivers pushed events to the registered callback, then stops after unlisten", async () => {
        const { internals, ws } = installWithServer();
        ws.open();

        const spy = vi.fn();
        const cbId = internals.transformCallback(spy);
        const eventId = (await internals.invoke("plugin:event|listen", {
            event: "pty:data:abc",
            handler: cbId,
        })) as number;

        ws.deliver({ type: "event", event: "pty:data:abc", payload: [1, 2, 3] });
        expect(spy).toHaveBeenCalledWith({ event: "pty:data:abc", id: cbId, payload: [1, 2, 3] });

        await internals.invoke("plugin:event|unlisten", { eventId });
        ws.deliver({ type: "event", event: "pty:data:abc", payload: [9] });
        expect(spy).toHaveBeenCalledTimes(1); // no further delivery
    });
});

describe("browser-environment commands (served locally, never over ws)", () => {
    it("clipboard_read / clipboard_write go through navigator.clipboard, not the socket", async () => {
        const { internals, ws } = installWithServer();
        ws.open();

        await internals.invoke("clipboard_write", { text: "hello" });
        expect((navigator.clipboard.writeText as any)).toHaveBeenCalledWith("hello");
        await expect(internals.invoke("clipboard_read")).resolves.toBe("hello");

        // Nothing was sent to the engine.
        expect(ws.sent).toHaveLength(0);
    });

    it("open_external_url opens http(s) urls and refuses other schemes", async () => {
        const { internals, ws } = installWithServer();
        ws.open();
        fakeWindow.open = vi.fn();

        await internals.invoke("open_external_url", { url: "https://example.com" });
        expect(fakeWindow.open).toHaveBeenCalledWith("https://example.com", "_blank", "noopener,noreferrer");

        // Off-Tauri now mirrors the desktop AppError wire shape so errMsg() localizes it.
        await expect(internals.invoke("open_external_url", { url: "file:///etc/passwd" })).rejects.toBe(
            `__rssh_err__|${JSON.stringify({ code: "window_non_https_url", params: { url: "file:///etc/passwd" } })}`,
        );
        expect(ws.sent).toHaveLength(0);
    });

    it("sftp_pick_folder rejects in a bare browser (no host bridge)", async () => {
        const { internals } = installWithServer();
        await expect(internals.invoke("sftp_pick_folder")).rejects.toMatch(/file_dialog_unavailable/);
    });

    it("sftp_pick_open_files uses the host bridge when present", async () => {
        const { internals } = installWithServer();
        fakeWindow.__RSSH_PICK__ = vi.fn(async (kind: string) =>
            kind === "files" ? ["/tmp/a.txt", "/tmp/b.txt"] : null,
        );
        await expect(internals.invoke("sftp_pick_open_files")).resolves.toEqual([
            { location: "/tmp/a.txt", name: "a.txt" },
            { location: "/tmp/b.txt", name: "b.txt" },
        ]);
        expect(fakeWindow.__RSSH_PICK__).toHaveBeenCalledWith("files");
    });

    it.each([
        ["/tmp/ report:final%20.txt", " report:final%20.txt"],
        ["/tmp/a\\b.txt", "a\\b.txt"],
        ["C:\\Downloads\\报告%20.txt", "报告%20.txt"],
        ["C:/Downloads/report:final.txt", "report:final.txt"],
    ])("preserves literal host filename metadata for %s", async (location, name) => {
        const { internals } = installWithServer();
        fakeWindow.__RSSH_PICK__ = vi.fn(async () => [location]);
        await expect(internals.invoke("sftp_pick_open_files"))
            .resolves.toEqual([{ location, name }]);
    });

    it("returns directory metadata without interpreting its name as a URI", async () => {
        const { internals, ws } = installWithServer();
        ws.open();
        fakeWindow.__RSSH_PICK__ = vi.fn(async () => "/tmp/备份%20:2026");
        await expect(internals.invoke("sftp_pick_folder", { write: false }))
            .resolves.toEqual({ location: "/tmp/备份%20:2026", name: "备份%20:2026" });
        expect(ws.sent).toHaveLength(0);
    });

    it("preserves cancellation from multi-file and directory choosers", async () => {
        const { internals } = installWithServer();
        fakeWindow.__RSSH_PICK__ = vi.fn(async () => null);
        await expect(internals.invoke("sftp_pick_open_files")).resolves.toBeNull();
        await expect(internals.invoke("sftp_pick_folder")).resolves.toBeNull();
    });

    it("sftp_pick_open_path returns one explicitly selected host file without using the server", async () => {
        const { internals, ws } = installWithServer();
        ws.open();
        fakeWindow.__RSSH_PICK__ = vi.fn(async () => ["/tmp/report.txt", "/tmp/other.txt"]);

        const picked = internals.invoke("sftp_pick_open_path");
        expect(ws.sent).toHaveLength(0);
        await expect(picked).resolves.toEqual({ location: "/tmp/report.txt", name: "report.txt" });
        expect(fakeWindow.__RSSH_PICK__).toHaveBeenCalledExactlyOnceWith("files");
    });

    it.each([{ files: null }, { files: [] }])("sftp_pick_open_path preserves host cancellation: $files", async ({ files }) => {
        const { internals, ws } = installWithServer();
        ws.open();
        fakeWindow.__RSSH_PICK__ = vi.fn(async () => files);

        const picked = internals.invoke("sftp_pick_open_path");
        expect(ws.sent).toHaveLength(0);
        await expect(picked).resolves.toBeNull();
    });

    it("sftp_pick_open_path rejects locally when a browser has no host chooser", async () => {
        const { internals, ws } = installWithServer();
        ws.open();

        const picked = internals.invoke("sftp_pick_open_path");
        expect(ws.sent).toHaveLength(0);
        await expect(picked).rejects.toMatch(/file_dialog_unavailable/);
    });

    it("builds an SFTP save path from the host folder picker", async () => {
        const { internals, ws } = installWithServer();
        fakeWindow.__RSSH_PICK__ = vi.fn(async (kind: string) =>
            kind === "folder" ? "/tmp/downloads" : null,
        );

        const picked = internals.invoke("sftp_pick_save_path", { defaultName: "report.txt" });
        ws.open();
        await Promise.resolve();

        expect(fakeWindow.__RSSH_PICK__).toHaveBeenCalledWith("folder");
        expect(ws.sent).toHaveLength(0);
        await expect(picked).resolves.toBe("/tmp/downloads/report.txt");
    });

    it.each([
        ["/tmp/a\\b", "/tmp/a\\b/report.txt"],
        ["C:\\Downloads", "C:\\Downloads\\report.txt"],
    ])("joins the save name using the host path style: %s", async (folder, expected) => {
        const { internals } = installWithServer();
        fakeWindow.__RSSH_PICK__ = vi.fn(async (kind: string) =>
            kind === "folder" ? folder : null,
        );

        await expect(internals.invoke("sftp_pick_save_path", { defaultName: "report.txt" }))
            .resolves.toBe(expected);
    });
});
