import { describe, expect, it } from "vitest";
import { Terminal } from "@xterm/xterm";
import { resetMouseTracking } from "./mouse-mode.ts";

function write(term: Terminal, data: string): Promise<void> {
    return new Promise((resolve) => term.write(data, resolve));
}

describe("resetMouseTracking", () => {
    it("turns off every mouse protocol supported by xterm", async () => {
        const term = new Terminal();

        await write(term, "\x1b[?1;1003;1006;2004h");
        expect(term.modes.mouseTrackingMode).toBe("any");
        expect(term.modes.applicationCursorKeysMode).toBe(true);
        expect(term.modes.bracketedPasteMode).toBe(true);

        await new Promise<void>((resolve) => resetMouseTracking(term, resolve));
        expect(term.modes.mouseTrackingMode).toBe("none");
        expect(term.modes.applicationCursorKeysMode).toBe(true);
        expect(term.modes.bracketedPasteMode).toBe(true);

        term.dispose();
    });
});
