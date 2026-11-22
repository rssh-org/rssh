import type { IBuffer, IMarker, Terminal } from "@xterm/xterm";
import type { TerminalExcerpt as ReportExcerpt } from "../stores/incident-reports.svelte.ts";
export type { ReportExcerpt };

export interface MenuReportCapture {
    excerpt: ReportExcerpt;
    releaseAutoSelection(): void;
}

type Point = Pick<MouseEvent, "clientX" | "clientY">;
type Source = Omit<ReportExcerpt, "text">;

// The terminal's capture listener and the pane menu receive the same DOM event.
// Each menu reads its own snapshot even when xterm has since changed its selection.
const menuCaptures = new WeakMap<MouseEvent, MenuReportCapture>();

export function reportCaptureForEvent(event: MouseEvent): MenuReportCapture | null {
    return menuCaptures.get(event) ?? null;
}

function pointedLine(terminal: Terminal, host: HTMLElement, point: Point | null) {
    const screen = host.querySelector(".xterm-screen")?.getBoundingClientRect();
    if (!point || !screen || screen.height <= 0 || screen.width <= 0) return null;
    const x = point.clientX - screen.left;
    const y = point.clientY - screen.top;
    if (x < 0 || x >= screen.width || y < 0 || y >= screen.height) return null;

    const buffer = terminal.buffer.active;
    const row = buffer.viewportY + Math.floor(y * terminal.rows / screen.height);
    return logicalLine(buffer, terminal.cols, row);
}

function logicalStart(buffer: IBuffer, row: number): number {
    while (row > 0 && buffer.getLine(row)?.isWrapped) row--;
    return row;
}

function logicalLine(buffer: IBuffer, cols: number, sourceRow: number) {
    let row = logicalStart(buffer, sourceRow);
    const start = row;
    let line = buffer.getLine(row);
    if (!line) return null;
    let text = "";
    let next = buffer.getLine(row + 1);
    while (next?.isWrapped) {
        // Match xterm's reflow rule: a wrapped row fills its width, except
        // for the empty cell left when a double-width glyph could not fit.
        const last = line.getCell(cols - 1);
        const widePadding = last?.getWidth() === 1 && last.getChars() === "" && next.getCell(0)?.getWidth() === 2;
        text += line.translateToString(false, 0, widePadding ? cols - 1 : cols);
        line = next;
        row++;
        next = buffer.getLine(row + 1);
    }
    return { row: start, text: text + line.translateToString(true, 0, cols) };
}

type SourceAnchor = { marker: IMarker; source: Source };

export function trackReportExcerpts(
    host: HTMLElement,
    terminal: Terminal,
    tabId: string,
    sessionName: () => string,
    isMac: boolean,
) {
    let pointer: Point | null = null;
    let press: { point: Point; capture: MenuReportCapture | null } | null = null;
    let currentId: string | undefined;
    let scrollbackEnds: SourceAnchor[] = [];
    let savedRows: (SourceAnchor & { text: string })[] = [];

    function archiveSession(id: string) {
        const buffer = terminal.buffer.normal;
        const cursor = buffer.baseY + buffer.cursorY;
        const live = new Set(terminal.markers);
        const source = { sessionId: id, sessionName: sessionName() };
        // One boundary covers all newly archived scrollback, even when it
        // contains thousands of lines. Repeated reconnects at the same
        // position keep the existing boundary.
        scrollbackEnds = scrollbackEnds.filter(end => live.has(end.marker));
        const lastEnd = scrollbackEnds[scrollbackEnds.length - 1]?.marker.line ?? -1;
        if (buffer.baseY - 1 > lastEnd) {
            const marker = terminal.registerMarker(buffer.baseY - 1 - cursor);
            if (marker) scrollbackEnds.push({ marker, source });
        }
        // Only the old screen can be overwritten in place by the new peer.
        // Reuse its unchanged rows; release anchors for text since erased.
        const claimed = new Set<number>();
        savedRows = savedRows.filter(saved => {
            if (!live.has(saved.marker)) return false;
            const row = saved.marker.line;
            if (row < buffer.baseY) return true;
            if (logicalLine(buffer, terminal.cols, row)?.text === saved.text) {
                claimed.add(row);
                return true;
            }
            saved.marker.dispose();
            return false;
        });
        for (let row = buffer.baseY; row < buffer.length; row++) {
            if (claimed.has(row) || buffer.getLine(row)?.isWrapped) continue;
            const line = logicalLine(buffer, terminal.cols, row);
            if (!line?.text) continue;
            const marker = terminal.registerMarker(row - cursor);
            if (marker) savedRows.push({ marker, source, text: line.text });
        }
    }

    function sourceAt(row: number | undefined): Source {
        const buffer = terminal.buffer.active;
        if ((savedRows.length || scrollbackEnds.length) && row !== undefined && buffer.type === "normal") {
            const start = logicalStart(buffer, row);
            // RIS replaces the buffer. Its old markers can still have a line
            // number even though they no longer belong to this terminal.
            const live = new Set(terminal.markers);
            const saved = savedRows.find(anchor => anchor.marker.line === start && live.has(anchor.marker));
            if (saved && logicalLine(buffer, terminal.cols, start)?.text === saved.text) return saved.source;
            const end = scrollbackEnds.find(anchor => start <= anchor.marker.line && live.has(anchor.marker));
            if (end) return end.source;
        }
        return { sessionId: currentId ?? tabId, sessionName: sessionName() };
    }

    function snapshot(point: Point | null): MenuReportCapture | null {
        const selection = terminal.getSelection();
        const line = selection
            ? { row: terminal.getSelectionPosition()?.start.y, text: selection }
            : pointedLine(terminal, host, point);
        if (!line?.text) return null;
        return {
            excerpt: { text: line.text, ...sourceAt(line.row) },
            releaseAutoSelection() {
                // xterm can select a word after our capture listener on macOS.
                // A second capture should only reuse a selection the user made.
                if (terminal.getSelection() !== selection) terminal.clearSelection();
            },
        };
    }

    function rememberPoint(event: MouseEvent) {
        pointer = { clientX: event.clientX, clientY: event.clientY };
    }
    function onMouseDown(event: MouseEvent) {
        const point = { clientX: event.clientX, clientY: event.clientY };
        pointer = point;
        // Firefox runs xterm's word selection on mousedown; macOS also
        // supports Control-click. Snapshot before either can replace a range.
        press = event.button === 2 || (isMac && event.ctrlKey && event.button === 0)
            ? { point, capture: snapshot(point) } : null;
    }
    function onMouseLeave() {
        pointer = null;
        press = null;
    }
    host.addEventListener("mousemove", rememberPoint);
    host.addEventListener("mousedown", onMouseDown, true);
    host.addEventListener("mouseleave", onMouseLeave);
    return {
        startSession(id: string) {
            if (currentId) archiveSession(currentId);
            currentId = id;
        },
        captureMenu(event: MouseEvent) {
            const capture = press && press.point.clientX === event.clientX && press.point.clientY === event.clientY
                ? press.capture : snapshot(event);
            press = null;
            if (capture) menuCaptures.set(event, capture);
        },
        forShortcut: () => snapshot(pointer)?.excerpt,
        dispose() {
            host.removeEventListener("mousemove", rememberPoint);
            host.removeEventListener("mousedown", onMouseDown, true);
            host.removeEventListener("mouseleave", onMouseLeave);
            for (const anchor of [...scrollbackEnds, ...savedRows]) anchor.marker.dispose();
        },
    };
}
