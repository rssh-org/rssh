// MUST be first — installs the IPC shim before any store/component module
// evaluates and touches invoke/listen (no-op inside the real Tauri webview).
import "./lib/ipc-boot.ts";
import App from "./App.svelte";
import { mount } from "svelte";
import * as theme from "./lib/themes/store.svelte.ts";
import * as transfers from "./lib/stores/transfers.svelte.ts";
import * as app from "./lib/stores/app.svelte.ts";
import { installLocalFileDropNavigationGuard } from "./lib/local-drop.ts"; /*防止WebView直接打开文件卡死*/
import { invoke } from "@tauri-apps/api/core";

installLocalFileDropNavigationGuard();
document.body.addEventListener("contextmenu", (event) => {
  if (!app.isMobile) event.preventDefault();
});

// Surface frontend errors to <app-data>/fe-errors.log so a frozen UI / editor
// that "won't open" can be diagnosed from the next run. Best-effort: the log
// write itself is fire-and-forget.
function logFrontendError(msg: string) {
  try {
    void invoke("log_frontend_error", { msg: msg.slice(0, 2000) });
  } catch {
    /* ignore */
  }
}
// If the render tree crashes (a component error takes the whole UI down), show
// a visible red bar with a reload button — the user is never stuck with "do
// nothing but kill the app".
function showErrorOverlay(msg: string) {
  try {
    let el = document.getElementById("fe-error-overlay");
    if (!el) {
      el = document.createElement("div");
      el.id = "fe-error-overlay";
      el.style.cssText =
        "position:fixed;left:0;right:0;bottom:0;z-index:999999;background:#b91c1c;color:#fff;font:12px/1.5 system-ui,sans-serif;padding:8px 12px;display:flex;align-items:center;gap:10px;";
      const btn = document.createElement("button");
      btn.textContent = "重载界面";
      btn.style.cssText =
        "background:#fff;color:#b91c1c;border:0;border-radius:4px;padding:4px 10px;font-weight:600;cursor:pointer;flex:none;";
      btn.onclick = () => location.reload();
      el.appendChild(btn);
      const span = document.createElement("span");
      span.style.cssText = "overflow:hidden;text-overflow:ellipsis;white-space:nowrap;min-width:0;";
      el.appendChild(span);
      document.body.appendChild(el);
    }
    const span = el.querySelector("span");
    if (span) span.textContent = "界面出错：" + msg.slice(0, 200);
  } catch {
    /* ignore */
  }
}
window.addEventListener("error", (event) => {
  const e = event.error;
  const msg = `${event.message} @ ${event.filename}:${event.lineno}:${event.colno}`;
  logFrontendError(`[error] ${msg}${e && e.stack ? "\n" + String(e.stack).slice(0, 1500) : ""}`);
  showErrorOverlay(msg);
});
window.addEventListener("unhandledrejection", (event) => {
  const r = event.reason;
  const msg = r instanceof Error ? `${r.message}\n${String(r.stack).slice(0, 1500)}` : String(r);
  logFrontendError(`[rejection] ${msg}`);
  showErrorOverlay(msg);
});
// Heartbeat for the Rust-side UI watchdog: if the JS main thread hangs (e.g.
// a Svelte effect runaway), this stops and the backend reloads the window.
setInterval(() => {
  try {
    void invoke("frontend_heartbeat").catch(() => {});
  } catch {
    /* ignore */
  }
}, 2000);

// Apply persisted theme before mount so first paint reflects the user's choice.
// We don't await — startup paint blocks on the persisted lookup otherwise. The
// :root literal defaults match the dark-neumorphism preset, so the worst case
// is a brief flicker if the user picked a different palette.
theme.init();

// SFTP 并发上限：从 DB 拉持久化值覆盖默认 10。fire-and-forget —— 用户在做出
// 第一笔 transfer 前这个 promise 已经 resolve；万一没（极快点击）也只是用一次默认值，
// 不影响功能。
void transfers.loadMaxConcurrent();

// Terminal interaction prefs (copy-on-select, right-click action, tab MRU).
// Global, loaded once at startup so the first terminal / tab switch honors
// persisted values without waiting for the Settings screen. fire-and-forget
// like loadMaxConcurrent.
void app.loadCopyOnSelect();
void app.loadRightClickAction();
void app.loadConfirmCloseTab();
void app.loadTabMru();
void app.loadCtrlRightClickMenu();

const instance = (async () => {
    const q = new URLSearchParams(window.location.search);
    // Standalone preview window: index.html?view=preview&sftp=...&path=...
    if (q.get("view") === "preview") {
        const { default: PreviewWindow } = await import("./lib/components/PreviewWindow.svelte");
        return mount(PreviewWindow, { target: document.getElementById("app")! });
    }
    // Standalone edit window (VS Code style): index.html?view=edit&sftp=...&path=...
    if (q.get("view") === "edit") {
        const { default: EditWindow } = await import("./lib/components/EditWindow.svelte");
        return mount(EditWindow, { target: document.getElementById("app")! });
    }
    return mount(App, { target: document.getElementById("app")! });
})();

export default instance;
