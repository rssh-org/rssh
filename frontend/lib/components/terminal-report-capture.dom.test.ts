import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { mount, unmount } from "svelte";
import { Terminal } from "@xterm/xterm";
import TerminalPane from "./TerminalPane.svelte";
import * as app from "../stores/app.svelte.ts";
import * as theme from "../themes/store.svelte.ts";
import * as runtime from "../stores/runtime.svelte.ts";
import * as keymap from "../stores/keymap.svelte.ts";
import { reportCaptureForEvent, trackReportExcerpts } from "../terminal/report-excerpt.ts";
import { addToReport } from "../terminal/add-to-report.ts";

vi.mock("../terminal/add-to-report.ts", () => ({ addToReport: vi.fn() }));
vi.mock("../stores/keymap.svelte.ts", async (importOriginal) => ({
    ...await importOriginal<typeof keymap>(),
    isMac: true,
}));
const invoke = vi.hoisted(() => vi.fn());
const listen = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@xterm/addon-image", () => ({ ImageAddon: class { activate() {} dispose() {} } }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class {
    activate() {} dispose() {} fit() {} proposeDimensions() { return undefined; }
} }));

const tabId = "local:report-capture";
const sessionName = "gateway-01";
const rect = { left: 20, top: 34, width: 120, height: 60 };
let sessionId: string;
let component: ReturnType<typeof mount> | undefined;
let terminal: Terminal;
let screen: HTMLElement;

beforeEach(async () => {
    vi.mocked(addToReport).mockClear();
    listen.mockReset().mockResolvedValue(() => {});
    invoke.mockReset().mockImplementation(async (command, args) => {
        if (command === "pty_spawn") return args.sessionId;
        if (command === "list_highlights") return [];
        if (command === "get_runtime_capabilities") return {
            ...runtime.capabilities(),
            terminalPolicy: { imageStorageLimitMb: 128, imagePixelLimit: 16_000_000,
                outputBacklogLimitBytes: 8_000_000, gpuRenderDefault: false },
        };
        return null;
    });
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(() =>
        new DOMRect(rect.left, rect.top, rect.width, rect.height));
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(256);
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(16);
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({
        clearRect() {}, fillRect() {},
    } as unknown as CanvasRenderingContext2D);
    const open = Terminal.prototype.open;
    vi.spyOn(Terminal.prototype, "open").mockImplementation(function (this: Terminal, host) {
        terminal = this;
        open.call(this, host);
    });
    await runtime.load();
    await theme.setTermGpuRender(false);
    app.addTab({ id: tabId, type: "local", label: sessionName });
    component = mount(TerminalPane, { target: document.body, props: { tabId, tabType: "local", meta: {} } });
    await vi.waitFor(() => expect(app.sessionIdForTab(tabId)).toBeTruthy());
    sessionId = app.sessionIdForTab(tabId)!;
    screen = document.querySelector(".xterm-screen")!;
    terminal.resize(10, 3);
});

afterEach(async () => {
    keymap.reset("term.addToReport");
    if (component) await unmount(component);
    component = undefined;
    app.closeTab(tabId);
    document.body.replaceChildren();
    vi.restoreAllMocks();
});

function write(text: string): Promise<void> {
    return new Promise(resolve => terminal.write(text, resolve));
}

function mouse(type: string, row: number, button = 0): MouseEvent {
    const event = new MouseEvent(type, {
        bubbles: true, cancelable: true, button,
        clientX: rect.left + 8, clientY: rect.top + (row + 0.5) * rect.height / terminal.rows,
    });
    screen.dispatchEvent(event);
    return event;
}

it("snapshots the pressed alternate-screen row before a word selection or redraw", async () => {
    await write("\x1b[?1049h\x1b[HGET /abc\r\nresponse\r\n(END)");
    mouse("mousedown", 0, 2);
    terminal.select(0, 0, 3);
    const menu = mouse("contextmenu", 0, 2);
    const capture = reportCaptureForEvent(menu)!;
    capture.releaseAutoSelection();
    expect(terminal.getSelection()).toBe("");
    expect(reportCaptureForEvent(mouse("contextmenu", 1, 2))?.excerpt.text).toBe("response");
    await write("\x1b[H\x1b[2Krepaint");
    app.updateTabLabel(tabId, "other label");
    expect(capture.excerpt).toEqual({ text: "GET /abc", sessionId, sessionName });
    expect(terminal.buffer.active.type).toBe("alternate");
});

it("preserves a partial multiline selection even when the pointer is on a different row", async () => {
    await write("0123456789\r\nabcdefghij\r\nlast");
    terminal.select(6, 0, 7);
    const menu = mouse("contextmenu", 2, 2);
    expect(reportCaptureForEvent(menu)?.excerpt.text).toBe("6789\nabc");
});

it("attributes retained scrollback, selections and the new alternate screen across a reconnect", async () => {
    terminal.resize(80, 6);
    await write("old request\r\nold response");
    await write("\r\n" + "retained\r\n".repeat(6) + "\x1b[?1049h\x1b[Hless request");
    expect(reportCaptureForEvent(mouse("contextmenu", 0, 2))?.excerpt.sessionId).toBe(sessionId);
    const notifyClose = listen.mock.calls.find(([event]) => event === `pty:close:${sessionId}`)![1];
    notifyClose();
    await vi.waitFor(() => expect(app.sessionIdForTab(tabId)).toBeUndefined());
    expect(terminal.buffer.active.type).toBe("alternate");
    expect(reportCaptureForEvent(mouse("contextmenu", 0, 2))?.excerpt.sessionId).toBe(sessionId);
    terminal.input("x");
    await vi.waitFor(() => {
        expect(app.sessionIdForTab(tabId)).toBeTruthy();
        expect(app.sessionIdForTab(tabId)).not.toBe(sessionId);
    });
    const newId = app.sessionIdForTab(tabId);
    await write("fresh response");
    app.updateTabLabel(tabId, "fresh tab name");
    terminal.scrollToTop();
    expect(reportCaptureForEvent(mouse("contextmenu", 0, 2))?.excerpt).toEqual({
        text: "old request", sessionId, sessionName,
    });
    terminal.select(4, 1, 8);
    expect(reportCaptureForEvent(mouse("contextmenu", 4, 2))?.excerpt).toEqual({
        text: "response", sessionId, sessionName,
    });
    terminal.clearSelection();
    terminal.scrollToBottom();
    const buffer = terminal.buffer.active;
    const newRow = buffer.baseY + buffer.cursorY - buffer.viewportY;
    expect(reportCaptureForEvent(mouse("contextmenu", newRow, 2))?.excerpt).toEqual({
        text: "fresh response", sessionId: newId, sessionName: "fresh tab name",
    });
    await write("\x1b[?1049h\x1b[Hnew pager");
    expect(reportCaptureForEvent(mouse("contextmenu", 0, 2))?.excerpt.sessionId).toBe(newId);
    await write("\x1b[?1049l\x1b[H\x1b[2Jafter clear");
    terminal.scrollToTop();
    expect(reportCaptureForEvent(mouse("contextmenu", 0, 2))?.excerpt).toEqual({
        text: "old request", sessionId, sessionName,
    });
    terminal.scrollToBottom();
    expect(reportCaptureForEvent(mouse("contextmenu", 0, 2))?.excerpt.sessionId).toBe(newId);
    await write("\x1bcafter reset");
    expect(reportCaptureForEvent(mouse("contextmenu", 0, 2))?.excerpt.sessionId).toBe(newId);
});

it("bounds reconnect anchors by the retained screen and clears them on disposal", async () => {
    const term = new Terminal({ allowProposedApi: true, cols: 12, rows: 4, scrollback: 1000 });
    const tracker = trackReportExcerpts(document.createElement("div"), term, "tab", () => "same name", true);
    const send = (text: string) => new Promise<void>(resolve => term.write(text, resolve));
    try {
        tracker.startSession("first");
        await send("retained\r\n".repeat(500));
        for (let i = 0; i < 10; i++) {
            // Replace a row without erasing its xterm marker; the other old
            // rows and the scrollback boundary stay at the same positions.
            await send(`\x1b[2;1Hattempt ${i}\x1b[4;1H`);
            tracker.startSession(String(i));
        }
        expect(term.markers.length).toBeLessThanOrEqual(term.rows + 1);
        await send("evict\r\n".repeat(1100));
        expect(term.markers).toHaveLength(0);
        tracker.startSession("latest");
    } finally {
        tracker.dispose();
        expect(term.markers).toHaveLength(0);
        term.dispose();
    }
});

it("joins the soft-wrapped line across the top of a scrolled viewport, preserving spaces and wide glyphs", async () => {
    await write("old\r\n123456789 日志\r\na\r\nb\r\nc\r\nd");
    terminal.scrollToLine(2);
    expect(terminal.buffer.active.viewportY).toBe(2);
    expect(terminal.buffer.active.getLine(2)?.isWrapped).toBe(true);
    expect(reportCaptureForEvent(mouse("contextmenu", 0, 2))?.excerpt.text).toBe("123456789 日志");
});

it("preserves tab-stop columns but skips the padding cell before a wrapped wide glyph", async () => {
    await write("A\t\t日志");
    expect(reportCaptureForEvent(mouse("contextmenu", 1, 2))?.excerpt.text).toBe("A        日志");
});

it("uses an explicitly rebound Option-arrow before the default shell word motion", async () => {
    const input = vi.fn();
    const data = terminal.onData(input);
    try {
        await write("log");
        mouse("mousemove", 0);
        const keydown = () => terminal.textarea!.dispatchEvent(new KeyboardEvent("keydown", {
            bubbles: true, cancelable: true, key: "ArrowLeft", altKey: true,
        }));
        keymap.setOverride("term.addToReport", { key: "ArrowLeft", alt: true });
        keydown();
        expect(addToReport).toHaveBeenCalledWith({ text: "log", sessionId, sessionName });
        expect(input).not.toHaveBeenCalled();
        keymap.reset("term.addToReport");
        keydown();
        expect(input).toHaveBeenCalledWith("\x1bb");
    } finally {
        data.dispose();
    }
});

it("intercepts a rebound shortcut on the terminal, keeping focus and the pager in place", async () => {
    keymap.setOverride("term.addToReport", { key: "g", ctrl: true, alt: true });
    await write("\x1b[?1049h\x1b[Hrequest\r\nresponse\r\n(END)");
    terminal.focus();
    mouse("mousemove", 1);
    const data = vi.fn();
    const sub = terminal.onData(data);
    try {
        const capture = () => terminal.textarea!.dispatchEvent(new KeyboardEvent("keydown", {
            bubbles: true, cancelable: true, key: "g", ctrlKey: true, altKey: true,
        }));
        expect(capture()).toBe(false);
        expect(addToReport).toHaveBeenLastCalledWith({ text: "response", sessionId, sessionName });
        terminal.select(1, 0, 3);
        capture();
        expect(addToReport).toHaveBeenLastCalledWith({ text: "equ", sessionId, sessionName });
        screen.closest(".xterm-host")!.dispatchEvent(new MouseEvent("mouseleave"));
        terminal.clearSelection();
        capture();
        expect(addToReport).toHaveBeenCalledTimes(2);
        expect(data).not.toHaveBeenCalled();
        expect(terminal.buffer.active.type).toBe("alternate");
        expect(document.activeElement).toBe(terminal.textarea);
    } finally {
        sub.dispose();
    }
});
