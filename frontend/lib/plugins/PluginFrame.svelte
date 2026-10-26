<script lang="ts">
    /**
     * One plugin iframe + the host side of the bridge.
     *
     * Security: sandbox="allow-scripts" only (no allow-same-origin — the
     * plugin gets an opaque origin and cannot touch app DOM/localStorage or
     * the Tauri IPC). Every message is validated against
     * `event.source === iframe.contentWindow` before it is trusted.
     *
     * Exec is brokered: request → invoke("plugin_exec") bound to this frame's
     * sessionId; the plugin never sees a session id. Visibility changes are
     * forwarded so well-behaved plugins can pause polling in background tabs.
     */
    import { invoke } from "@tauri-apps/api/core";
    import {
        isPluginRequest,
        execResponseOk,
        execResponseErr,
        helloEvent,
        visibilityEvent,
        readThemeTokens,
        MAX_CONCURRENT_EXEC,
        type ExecResult,
        type SizeReport,
    } from "../plugins/bridge.ts";
    import type { PluginInfo } from "../plugins/store.svelte.ts";

    let { plugin, src, sessionId, visible, onSize }: {
        plugin: PluginInfo;
        src: string;
        sessionId: string;
        visible: boolean;
        /** Content size notifications — the region sizes this frame's
         *  container from them (height in the side column, width in the strip). */
        onSize?: (size: SizeReport) => void;
    } = $props();

    let iframeEl = $state<HTMLIFrameElement | null>(null);
    let frameLoaded = $state(false);
    let inFlight = 0;

    function post(msg: unknown): void {
        iframeEl?.contentWindow?.postMessage(msg, "*");
    }

    /** Backend rejects carry `__rssh_err__|{code,...}`; surface the code to
     *  the plugin and keep the raw text as the human message. */
    function parseError(e: unknown): { code: string; message: string } {
        const s = e == null ? "" : e instanceof Error ? e.message : String(e);
        const PREFIX = "__rssh_err__|";
        if (s.startsWith(PREFIX)) {
            try {
                const payload = JSON.parse(s.slice(PREFIX.length));
                return { code: String(payload.code ?? "plugin_exec_failed"), message: s };
            } catch { /* fall through */ }
        }
        return { code: "plugin_exec_failed", message: s };
    }

    function onMessage(e: MessageEvent): void {
        if (!iframeEl || e.source !== iframeEl.contentWindow) return;
        if (!isPluginRequest(e.data)) return;
        const req = e.data;
        if (req.cmd === "size") {
            onSize?.(req.payload);
            return;
        }
        if (inFlight >= MAX_CONCURRENT_EXEC) {
            post(execResponseErr(req.id, "plugin_busy", "too many concurrent execs"));
            return;
        }
        inFlight++;
        invoke<ExecResult>("plugin_exec", {
            sessionId,
            command: req.payload.command,
            timeoutMs: req.payload.timeoutMs,
        })
            .then(result => post(execResponseOk(req.id, result)))
            .catch(err => {
                const { code, message } = parseError(err);
                post(execResponseErr(req.id, code, message));
            })
            .finally(() => { inFlight--; });
    }

    function onFrameLoad(): void {
        frameLoaded = true;
        // The theme rides along with hello — the sandbox hides the host
        // stylesheet, tokens are the only colors a plugin ever sees.
        post(helloEvent(readThemeTokens(getComputedStyle(document.documentElement))));
        post(visibilityEvent(visible));
    }

    // message events fire on window, not the iframe element
    $effect(() => {
        window.addEventListener("message", onMessage);
        return () => window.removeEventListener("message", onMessage);
    });

    $effect(() => {
        // Track `visible`; once the document is up, tell the plugin so it can
        // stop polling while its tab is inactive.
        const v = visible;
        if (frameLoaded) post(visibilityEvent(v));
    });
</script>

<iframe
    bind:this={iframeEl}
    class="plugin-frame"
    title={plugin.name}
    {src}
    sandbox="allow-scripts"
    onload={onFrameLoad}
></iframe>

<style>
    .plugin-frame {
        width: 100%;
        height: 100%;
        border: none;
        display: block;
        background: transparent;
    }
</style>
