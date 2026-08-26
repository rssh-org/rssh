// Re-record welcome-*.gif from the live site scenes. The site lives in the
// sibling rssh-docs checkout (github.com/rssh-org/docs, serves rssh.ofcoder.com);
// this script drives its index.html + scenes.js. Headless Chrome over raw CDP —
// no puppeteer needed — captures Page.screencast frames for exactly ONE loop of
// each scene, aligned to the player's own reset boundary so every GIF starts
// from the empty/initial state and ends on the final-state hold.
//
//     node scripts/record-welcome-gifs.mjs [scene ...]   // e.g. "ai sync"
//
// Output: welcome-<scene>.gif in the docs repo root — 1280×800 @ 20fps, same
// filenames the site and READMEs already reference (in-place refresh, zero doc
// edits).
import { spawn } from "node:child_process";
import { mkdtemp, rm, copyFile, mkdir, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const DOCS = process.env.RSSH_DOCS_DIR || path.resolve(ROOT, "..", "rssh-docs");
const PAGE_URL = "file://" + path.join(DOCS, "index.html");
const CHROME = process.env.CHROME_BIN || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const PORT = 9333;
const VIEW = { width: 1300, height: 1100 }; // scene = min(1020px, 92vw) wide
const FPS = 20;
const OUT_SIZE = { w: 1280 };

// Keep in sync with SCENES[*].loop in the docs repo's scenes.js — the player restarts
// itself every `loop` ms, which is both our record duration and our trim mark.
const SCENES = {
    ai: 9600,
    blocks: 12100,
    discovery: 11000,
    sync: 8700,
    cli: 8900,
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/* ── minimal CDP client over the builtin WebSocket ─────────────────── */

class Cdp {
    constructor(ws) {
        this.ws = ws;
        this.seq = 0;
        this.pending = new Map();
        this.listeners = new Map();
        ws.addEventListener("message", (ev) => {
            const msg = JSON.parse(ev.data);
            if (msg.id !== undefined) {
                const p = this.pending.get(msg.id);
                if (!p) return;
                this.pending.delete(msg.id);
                msg.error ? p.reject(new Error(msg.error.message)) : p.resolve(msg.result);
            } else {
                for (const fn of this.listeners.get(msg.method) ?? []) fn(msg.params);
            }
        });
    }

    static async connect(url) {
        const ws = new WebSocket(url);
        await new Promise((resolve, reject) => {
            ws.addEventListener("open", resolve, { once: true });
            ws.addEventListener("error", reject, { once: true });
        });
        return new Cdp(ws);
    }

    send(method, params = {}) {
        const id = ++this.seq;
        return new Promise((resolve, reject) => {
            this.pending.set(id, { resolve, reject });
            this.ws.send(JSON.stringify({ id, method, params }));
        });
    }

    on(method, fn) {
        if (!this.listeners.has(method)) this.listeners.set(method, []);
        this.listeners.get(method).push(fn);
        return () => {
            const list = this.listeners.get(method);
            list.splice(list.indexOf(fn), 1);
        };
    }
}

// Evaluate an expression and parse its JSON value; throws on page errors.
async function evalJson(cdp, expression) {
    const r = await cdp.send("Runtime.evaluate", { expression, returnByValue: true });
    if (r.exceptionDetails) throw new Error(`page error: ${r.exceptionDetails.text} in ${expression}`);
    return JSON.parse(r.result.value);
}

async function poll(cdp, expression, timeoutMs, label) {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
        if (await evalJson(cdp, expression)) return;
        if (Date.now() > deadline) throw new Error(`timeout waiting for ${label}`);
        await sleep(120);
    }
}

/* ── in-page instrumentation ───────────────────────────────────────── */

// Reset probes: each returns true exactly on the transition that only the
// player's reset() performs at a loop boundary — typed text going from full
// back to empty, all `.block`s losing `lit`, or the fig losing its first
// state class. Everything else (mid-loop select/deselect, typing) stays mute.
const INSTRUMENT = `
(() => {
    const probeTyped = (fig) => {
        const el = fig.querySelector('[data-typed]');
        let had = false;
        return () => { const full = el.textContent !== ""; const hit = had && !full; had = full; return hit; };
    };
    const probeLit = (fig) => {
        const blocks = [...fig.querySelectorAll('.block')];
        let all = false;
        return () => { const now = blocks.every((b) => b.classList.contains("lit")); const hit = all && !now; all = now; return hit; };
    };
    const probeFigClass = (fig, cls) => {
        let had = false;
        return () => { const now = fig.classList.contains(cls); const hit = had && !now; had = now; return hit; };
    };
    const PROBE = { ai: probeTyped, blocks: probeLit, discovery: probeTyped, cli: probeTyped, sync: (f) => probeFigClass(f, "s-nodes") };
    window.__resets = {};
    for (const fig of document.querySelectorAll("[data-scene]")) {
        const name = fig.dataset.scene;
        const log = (window.__resets[name] = []);
        const check = PROBE[name](fig);
        new MutationObserver(() => { if (check()) log.push(performance.now()); })
            .observe(fig, { subtree: true, attributes: true, attributeFilter: ["class"], childList: true });
    }
    return JSON.stringify(Object.keys(window.__resets));
})()`;

/* ── capture one scene ─────────────────────────────────────────────── */

async function recordScene(cdp, name, loop) {
    const tmp = await mkdtemp(path.join(os.tmpdir(), `rssh-gif-${name}-`));
    const raw = path.join(tmp, "raw");
    const seq = path.join(tmp, "seq");
    await mkdir(raw);
    await mkdir(seq);

    // Instant scroll (override any CSS smooth-scroll) so the IntersectionObserver
    // (threshold 0.3) starts the player.
    await evalJson(cdp, `(() => {
        const fig = document.querySelector('figure[data-scene="${name}"]');
        const r = fig.getBoundingClientRect();
        window.scrollTo({ top: Math.max(0, r.top + window.scrollY - (innerHeight - r.height) / 2), behavior: "instant" });
        return true;
    })()`);
    await sleep(300);

    // Wait for one natural restart: proves the player booted and the page is warm.
    await poll(cdp, `(window.__resets["${name}"] || []).length >= 1`, loop * 3 + 10000, `${name} first restart`);

    // Sleep until just before the NEXT restart, then watch for it live.
    const plan = await evalJson(cdp, `JSON.stringify({
        last: window.__resets["${name}"].at(-1),
        perf: performance.now(),
    })`);
    const wait = plan.last + loop - plan.perf - 900;
    if (wait > 0) await sleep(wait);

    // Collect screencast frames (device pixels) until the loop after the next
    // reset. Frames go straight to disk — a full loop of 2600×2200 PNGs does
    // not belong in memory.
    const frames = [];
    const writes = [];
    let firstPng = null;
    const unlisten = cdp.on("Page.screencastFrame", (f) => {
        const file = path.join(raw, `f${String(frames.length).padStart(5, "0")}.png`);
        frames.push({ wall: Date.now(), ts: f.metadata?.timestamp ?? null, file });
        if (!firstPng) firstPng = Buffer.from(f.data, "base64");
        writes.push(writeFile(file, Buffer.from(f.data, "base64")));
        cdp.send("Page.screencastFrameAck", { sessionId: f.sessionId }).catch(() => {});
    });
    await cdp.send("Page.startScreencast", { format: "png", everyNthFrame: 1 });

    await poll(cdp, `window.__resets["${name}"].length >= 2`, loop + 5000, `${name} aligned restart`);
    const mark = await evalJson(cdp, `JSON.stringify({
        t: window.__resets["${name}"].at(-1),
        off: Date.now() - performance.now(),
    })`);
    const t0 = mark.t + mark.off; // loop start, wall clock — same machine, same clock

    const rect = await evalJson(cdp, `(() => {
        // Crop to what is actually VISIBLE, not the stage box: the sync
        // diagram overflows .stage on the right, while the ai panel's layout
        // box overflows its overflow:hidden window (gBCR ignores clipping).
        // So: clip every element to its nearest clipping ancestor, then union.
        // The parked mock cursor (left/top at 108%) is excluded outright.
        // Measured at the end of the loop — final state is widest and settled.
        const fig = document.querySelector('figure[data-scene="${name}"]');
        const clipped = new WeakMap();
        let l = Infinity, t = Infinity, r = -Infinity, b = -Infinity;
        for (const el of fig.querySelectorAll('*')) {
            if (el.closest('.mock-cursor')) continue;
            let x = el.getBoundingClientRect();
            if (!x.width && !x.height) continue;
            let box = clipped.get(el.parentElement);
            if (box === undefined) {
                box = null;
                for (let p = el.parentElement; p && p !== fig.parentElement; p = p.parentElement) {
                    const cs = getComputedStyle(p);
                    if (/^(hidden|clip|auto|scroll)$/.test(cs.overflowX) || /^(hidden|clip|auto|scroll)$/.test(cs.overflowY)) {
                        box = p.getBoundingClientRect();
                        break;
                    }
                }
                clipped.set(el.parentElement, box);
            }
            if (box) {
                x = {
                    left: Math.max(x.left, box.left), top: Math.max(x.top, box.top),
                    right: Math.min(x.right, box.right), bottom: Math.min(x.bottom, box.bottom),
                };
                if (x.right <= x.left || x.bottom <= x.top) continue;
            }
            l = Math.min(l, x.left); t = Math.min(t, x.top);
            r = Math.max(r, x.right); b = Math.max(b, x.bottom);
        }
        const s = fig.querySelector('.stage').getBoundingClientRect();
        l = Math.min(l, s.left); t = Math.min(t, s.top);
        r = Math.max(r, s.right); b = Math.max(b, s.bottom);
        return JSON.stringify({ x: l, y: t, width: r - l, height: b - t,
            stage: { x: s.left, y: s.top, width: s.width, height: s.height } });
    })()`);
    while (Date.now() < t0 + loop + 350) await sleep(50);
    await cdp.send("Page.stopScreencast");
    unlisten();
    await Promise.all(writes);

    // Calibrate the CSS→pixel scale from reality: first frame's pixel width
    // over the page's innerWidth. Flags lie; pixels don't.
    const scale = firstPng.readUInt32BE(16) / await evalJson(cdp, "innerWidth");

    // Resample onto a fixed FPS grid: the first frame at or after each tick,
    // so the GIF opens on the post-reset state (never the old loop's tail).
    // Frame times are the PRESENTATION timestamps screencast carries (wall
    // clock, seconds) — receipt time would let pre-reset composites leak
    // into frame 0, since PNG-encoding a big surface delays delivery well
    // past the moment its pixels were composited.
    const hasTs = frames.some((f) => f.ts != null);
    const timeOf = (f) => (hasTs ? f.ts * 1000 : f.wall);
    const epsilon = hasTs ? 45 : 400; // no ts → fall back to a stale-frame drop
    const live = frames.filter((f) => timeOf(f) >= t0 + epsilon);
    if (frames.length > 2) {
        const span = (timeOf(frames.at(-1)) - timeOf(frames[0])) / 1000;
        console.log(`[screencast] ${frames.length} frames over ${span.toFixed(1)}s = ${(frames.length / span).toFixed(1)} fps delivered`);
    }
    let n = 0;
    for (let k = 0; k * 1000 < loop; k += 1 / FPS) {
        const src = live.find((f) => timeOf(f) >= t0 + k * 1000 - epsilon) ?? live.at(-1);
        await copyFile(src.file, path.join(seq, `f${String(n++).padStart(5, "0")}.png`));
    }

    return { tmp, seq, count: n, rect, scale, loop };
}

/* ── encode ────────────────────────────────────────────────────────── */

function encode(scene, cap) {
    const { x, y, width, height } = cap.rect;
    const crop = [
        Math.round(width * cap.scale), Math.round(height * cap.scale),
        Math.max(0, Math.round(x * cap.scale)), Math.max(0, Math.round(y * cap.scale)),
    ].join(":");
    const vf = [
        `crop=${crop}`,
        `scale=${OUT_SIZE.w}:-2:flags=lanczos`, // height follows the scene's own aspect
        "split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle",
    ].join(",");
    const out = path.join(DOCS, `welcome-${scene}.gif`);
    if (process.env.DEBUG) console.log(`[rect] ${JSON.stringify(cap.rect)} crop=${crop}`);
    const args = ["-y", "-loglevel", "error", "-framerate", String(FPS), "-i", path.join(cap.seq, "f%05d.png"),
        "-vf", vf, "-loop", "0", out];
    return new Promise((resolve, reject) => {
        const ff = spawn("ffmpeg", args, { stdio: ["ignore", "ignore", "inherit"] });
        ff.on("close", (code) => (code === 0 ? resolve(out) : reject(new Error(`ffmpeg exited ${code} for ${scene}`))));
    });
}

/* ── main ──────────────────────────────────────────────────────────── */

const wanted = process.argv.slice(2).filter((s) => s in SCENES);
const names = wanted.length ? wanted : Object.keys(SCENES);

const profile = await mkdtemp(path.join(os.tmpdir(), "rssh-chrome-"));
const chrome = spawn(CHROME, [
    "--headless=new", `--remote-debugging-port=${PORT}`, `--user-data-dir=${profile}`,
    // window-size is CSS px. Ask for a 2× device scale so text renders with
    // retina sharpness before the 1280-wide downscale — but the recorder
    // measures the real scale from the first frame instead of trusting the
    // flag (it silently rounded 1.5→2 on this machine's headless build).
    "--force-device-scale-factor=2", `--window-size=${VIEW.width},${VIEW.height}`,
    "--hide-scrollbars", "--no-first-run", "--no-default-browser-check", "about:blank",
], { stdio: "ignore" });

try {
    const version = await (async () => {
        const deadline = Date.now() + 15000;
        for (;;) {
            try {
                const r = await fetch(`http://127.0.0.1:${PORT}/json/version`);
                return r.json();
            } catch {
                if (Date.now() > deadline) throw new Error("Chrome DevTools endpoint never came up");
                await sleep(200);
            }
        }
    })();
    // Create the target blank, THEN navigate — createTarget-with-URL races and
    // fakes "script missing" on file:// pages.
    const created = await (await fetch(`http://127.0.0.1:${PORT}/json/new?about:blank`, { method: "PUT" })).json();
    const cdp = await Cdp.connect(created.webSocketDebuggerUrl);
    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");
    await cdp.send("Page.navigate", { url: PAGE_URL });
    await poll(cdp, `document.readyState === "complete"`, 15000, "page load");
    await poll(cdp, `!!document.querySelector('figure[data-scene="ai"] .stage')`, 10000, "scenes boot");
    await evalJson(cdp, INSTRUMENT);

    for (const name of names) {
        process.stdout.write(`recording ${name} (${SCENES[name]}ms loop)… `);
        const cap = await recordScene(cdp, name, SCENES[name]);
        const out = await encode(name, cap);
        if (!process.env.DEBUG) await rm(cap.tmp, { recursive: true, force: true });
        else console.log(`[debug] raw frames kept: ${cap.tmp}`);
        console.log(`${cap.count} frames → ${path.relative(ROOT, out)}`);
    }
    console.log(`chrome ${version.Browser.split(" ").pop()} — done`);
} finally {
    chrome.kill();
    await rm(profile, { recursive: true, force: true }).catch(() => {});
}
