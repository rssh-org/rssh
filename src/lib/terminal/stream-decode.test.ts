import { describe, expect, it } from "vitest";

import { decodeStreamChunk } from "./stream-decode";

describe("decodeStreamChunk", () => {
    it("round-trips ASCII bytes", () => {
        const bytes = new TextEncoder().encode("hello terminal\r\n");
        const b64 = btoa(String.fromCharCode(...bytes));
        expect(decodeStreamChunk(b64)).toEqual(bytes);
    });

    it("round-trips binary bytes with high bit set", () => {
        const bytes = Uint8Array.from([0, 1, 2, 250, 251, 252, 253, 254, 255]);
        const b64 = btoa(String.fromCharCode(...bytes));
        expect(decodeStreamChunk(b64)).toEqual(bytes);
    });

    it("decodes empty payload", () => {
        expect(decodeStreamChunk("")).toEqual(new Uint8Array(0));
    });

    it("decodes padded base64 of arbitrary length", () => {
        for (const n of [1, 2, 3, 4, 5, 63, 64, 65]) {
            const bytes = new Uint8Array(n);
            for (let i = 0; i < n; i++) bytes[i] = (i * 37 + 11) % 256;
            const b64 = btoa(String.fromCharCode(...bytes));
            expect(decodeStreamChunk(b64)).toEqual(bytes);
        }
    });
});
