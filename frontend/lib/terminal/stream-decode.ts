/** Backend terminal-byte events (`ssh:data` / `pty:data` / `telnet:data` /
 * `serial:data`) carry base64 text — a JSON number array costs ~4 bytes per
 * output byte on the IPC bridge. Decode natively when the runtime ships
 * Uint8Array.fromBase64 (Chromium 140+), atob loop otherwise. */
export function decodeStreamChunk(b64: string): Uint8Array {
    const fromBase64 = (Uint8Array as unknown as {
        fromBase64?: (s: string) => Uint8Array;
    }).fromBase64;
    if (typeof fromBase64 === "function") return fromBase64(b64);
    const bin = atob(b64);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
}
