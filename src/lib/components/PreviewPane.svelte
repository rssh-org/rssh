<script lang="ts">
    // Remote file preview pane, shared by the SFTP modal and the standalone
    // preview window. Downloads the file via the existing `sftp_download`
    // command and renders it by kind: image / PDF / rendered markdown /
    // CSV·TSV table / syntax-highlighted CodeMirror view with Ctrl+F search.
    import { onDestroy, onMount } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import { EditorView, basicSetup } from "codemirror";
    import { EditorState } from "@codemirror/state";
    import { StreamLanguage } from "@codemirror/language";
    import { oneDark } from "@codemirror/theme-one-dark";
    import { shell } from "@codemirror/legacy-modes/mode/shell";
    import { python } from "@codemirror/legacy-modes/mode/python";
    import { r } from "@codemirror/legacy-modes/mode/r";
    import { perl } from "@codemirror/legacy-modes/mode/perl";
    import { rust } from "@codemirror/legacy-modes/mode/rust";
    import { go } from "@codemirror/legacy-modes/mode/go";
    import { javascript } from "@codemirror/legacy-modes/mode/javascript";
    import { clike } from "@codemirror/legacy-modes/mode/clike";
    import { css } from "@codemirror/legacy-modes/mode/css";
    import { sql } from "@codemirror/legacy-modes/mode/sql";
    import { xml } from "@codemirror/legacy-modes/mode/xml";
    import { yaml } from "@codemirror/legacy-modes/mode/yaml";
    import { toml } from "@codemirror/legacy-modes/mode/toml";
    import { properties } from "@codemirror/legacy-modes/mode/properties";
    import { powerShell } from "@codemirror/legacy-modes/mode/powershell";
    import { renderMarkdown } from "../ai/markdown.ts";
    import { errMsg, t } from "../i18n/index.svelte.ts";
    import {
        cmLanguageFor, decodeText, mediaMime, parseDelimited, previewKind,
        PREVIEW_CM_MAX, PREVIEW_MEDIA_MAX, PREVIEW_TABLE_MAX_COLS,
        PREVIEW_TABLE_MAX_ROWS, PREVIEW_TEXT_MAX,
        type PreviewKind,
    } from "../sftp-preview.ts";

    let {
        name,
        sftpId,
        path,
        size,
        onClose,
        onOpenWindow,
    }: {
        name: string;
        sftpId: string;
        path: string;
        size?: number;
        onClose: () => void;
        onOpenWindow?: () => void;
    } = $props();

    const MODES: Record<string, unknown> = {
        shell, python, r, perl, rust, go, javascript, clike,
        css, sql, xml, yaml, toml, properties, powerShell,
    };

    let kind = $state<PreviewKind | null>(null);
    let loading = $state(true);
    let error = $state("");
    let truncated = $state(false);
    let mediaUrl = $state("");
    let text = $state("");
    let mdHtml = $state("");
    let table = $state<string[][] | null>(null);
    let tableQuery = $state("");
    let mdQuery = $state("");
    let cmEl = $state<HTMLDivElement | null>(null);
    let view: EditorView | null = null;

    function formatSize(n: number): string {
        if (n >= 1024 * 1024 * 1024) return (n / 1024 / 1024 / 1024).toFixed(1) + " GB";
        if (n >= 1024 * 1024) return (n / 1024 / 1024).toFixed(1) + " MB";
        if (n >= 1024) return (n / 1024).toFixed(1) + " KB";
        return n + " B";
    }

    async function fetchBytes(cap: number, allowTruncate: boolean): Promise<Uint8Array | null> {
        if (size !== undefined && size > cap && !allowTruncate) {
            error = t("sftp.preview.too_large", { size: formatSize(cap) });
            return null;
        }
        // Slow remote storage can hang a read forever — bound the wait so the
        // user gets an error they can retry instead of an endless spinner.
        const arr = await withTimeout(invoke<number[]>("sftp_download", { sftpId, path }), 30_000);
        if (loadAbort) return null;
        const bytes = new Uint8Array(arr.length);
        for (let i = 0; i < arr.length; i++) bytes[i] = arr[i];
        if (bytes.length > cap) {
            if (allowTruncate) {
                truncated = true;
                return bytes.subarray(0, cap);
            }
            error = t("sftp.preview.too_large", { size: formatSize(cap) });
            return null;
        }
        return bytes;
    }

    function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
        return new Promise<T>((resolve, reject) => {
            const timer = window.setTimeout(() => {
                reject(new Error("rssh-load-timeout"));
            }, ms);
            p.then(
                (v) => { clearTimeout(timer); resolve(v); },
                (e) => { clearTimeout(timer); reject(e); },
            );
        });
    }

    let loadAbort = false;

    async function load() {
        error = "";
        loading = true;
        void invoke("log_frontend_error", { msg: `[event] preview load start ${path}` }).catch(() => {});
        const k = previewKind(name);
        if (!k) { error = t("sftp.preview.unsupported"); loading = false; return; }
        kind = k;
        try {
            if (k === "image" || k === "pdf") {
                const bytes = await fetchBytes(PREVIEW_MEDIA_MAX, false);
                if (!bytes) return;
                mediaUrl = URL.createObjectURL(new Blob([bytes], { type: mediaMime(name) }));
            } else {
                const bytes = await fetchBytes(PREVIEW_TEXT_MAX, true);
                if (!bytes) return;
                void invoke("log_frontend_error", { msg: `[event] preview load got ${bytes.length} bytes` }).catch(() => {});
                const txt = decodeText(bytes);
                if (k === "table") {
                    const delim = /\.tsv$/i.test(name) ? "\t" : ",";
                    let rows = parseDelimited(txt, delim);
                    if (rows.length > PREVIEW_TABLE_MAX_ROWS + 1) {
                        rows = rows.slice(0, PREVIEW_TABLE_MAX_ROWS + 1);
                        truncated = true;
                    }
                    rows = rows.map((r) => r.slice(0, PREVIEW_TABLE_MAX_COLS));
                    table = rows;
                } else if (k === "markdown") {
                    mdHtml = renderMarkdown(txt);
                } else {
                    text = txt;
                }
            }
        } catch (e: any) {
            if (loadAbort) return;
            const msg = e && e.message === "rssh-load-timeout"
                ? t("sftp.editor.load_timeout")
                : t("sftp.editor.load_failed", { err: errMsg(e) });
            void invoke("log_frontend_error", { msg: `[event] preview load failed: ${msg}` }).catch(() => {});
            error = msg;
        }
        loading = false;
    }

    onMount(() => { void load(); });

    // Mount the CodeMirror editor once text for a code-kind file is available.
    $effect(() => {
        if (kind !== "code" || !cmEl || view) return;
        const mode = cmLanguageFor(name);
        const lang = MODES[mode];
        const extensions: any[] = [
            basicSetup,
            oneDark,
            EditorState.readOnly.of(true),
            EditorView.lineWrapping,
            EditorView.theme({
                "&": { height: "100%", fontSize: "12.5px" },
                ".cm-scroller": { fontFamily: "var(--term-font, Consolas, 'Cascadia Mono', monospace)", lineHeight: "1.45" },
                ".cm-gutters": { backgroundColor: "transparent" },
            }),
        ];
        if (lang) extensions.push(StreamLanguage.define(lang as any));
        try {
            view = new EditorView({
                state: EditorState.create({ doc: text.slice(0, PREVIEW_CM_MAX), extensions }),
                parent: cmEl,
            });
        } catch (e: any) {
            error = t("sftp.editor.view_failed", { err: errMsg(e) });
        }
    });

    onDestroy(() => {
        loadAbort = true;
        view?.destroy();
        view = null;
        if (mediaUrl) URL.revokeObjectURL(mediaUrl);
    });

    // ── table filtering ──
    const tableRows = $derived(table ? table.slice(1) : []);
    const filteredRows = $derived.by(() => {
        if (!tableQuery.trim()) return tableRows;
        const q = tableQuery.trim().toLowerCase();
        return tableRows.filter((row) => row.some((c) => c.toLowerCase().includes(q)));
    });

    // ── markdown in-page find (WebView2 supports window.find) ──
    function findInMd() {
        if (!mdQuery) return;
        (window as any).find(mdQuery, false, false, true);
    }
</script>

<div class="preview-pane">
    <div class="preview-head">
        <span class="preview-name" title={name}>{name}</span>
        <div class="preview-actions">
            {#if onOpenWindow}
                <button type="button" class="btn-icon" onclick={onOpenWindow}
                    title={t("sftp.ctx.open_window")} aria-label={t("sftp.ctx.open_window")}>▣</button>
            {/if}
            <button type="button" class="btn-icon" onclick={onClose}
                title={t("common.close")} aria-label={t("common.close")}>×</button>
        </div>
    </div>

    {#if loading}
        <p class="loading">{t("common.loading")}</p>
    {:else if error}
        <div class="preview-error">
            <p>{error}</p>
            <div class="preview-error-actions">
                <button type="button" class="btn btn-sm" onclick={load}>{t("sftp.editor.retry")}</button>
                <button type="button" class="btn btn-sm" onclick={onClose}>{t("sftp.editor.back")}</button>
            </div>
        </div>
    {:else if kind === "image"}
        <div class="media-body">
            <img class="preview-img" src={mediaUrl} alt={name} />
        </div>
    {:else if kind === "pdf"}
        <iframe class="preview-frame" src={mediaUrl} title={name}></iframe>
    {:else if kind === "markdown"}
        <div class="md-toolbar">
            <input class="md-find" type="text" placeholder={t("sftp.preview.md_find")}
                bind:value={mdQuery} onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); findInMd(); } }} />
            <button type="button" class="btn btn-sm" onclick={findInMd}>{t("sftp.preview.md_find_next")}</button>
        </div>
        <div class="md-body">{@html mdHtml}</div>
    {:else if kind === "table"}
        <div class="table-toolbar">
            <input class="table-filter" type="text" placeholder={t("sftp.preview.table_filter")}
                bind:value={tableQuery} />
            <span class="table-count">{t("sftp.preview.rows", { shown: filteredRows.length, total: tableRows.length })}</span>
        </div>
        <div class="table-wrap">
            <table class="data-table">
                {#if table && table.length > 0}
                    <thead>
                        <tr>{#each table[0] as h, i}<th title={h}>{h}</th>{/each}</tr>
                    </thead>
                {/if}
                <tbody>
                    {#each filteredRows as row}
                        <tr>{#each row as cell}<td title={cell}>{cell}</td>{/each}</tr>
                    {/each}
                </tbody>
            </table>
        </div>
    {:else}
        <div class="code-wrap">
            <div class="cm-host" bind:this={cmEl}></div>
            <p class="preview-note">{t("sftp.preview.find_hint")}</p>
        </div>
    {/if}

    {#if truncated}
        <p class="preview-note truncated-note">{t("sftp.preview.truncated", { size: formatSize(PREVIEW_TEXT_MAX) })}</p>
    {/if}
</div>

<style>
    .preview-pane {
        display: flex;
        flex-direction: column;
        height: 100%;
        min-height: 0;
    }

    .preview-head {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        margin-bottom: 10px;
        flex: none;
    }

    .preview-name {
        font-size: 13px;
        font-weight: 600;
        color: var(--text);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        min-width: 0;
    }

    .preview-actions {
        display: flex;
        gap: 6px;
        flex: none;
    }

    .loading { color: var(--text-secondary, #6b7280); }

    .preview-error {
        color: var(--danger, #d64545);
        padding: 8px 0;
        font-size: 13px;
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 10px;
    }

    .preview-error-actions {
        display: flex;
        gap: 8px;
    }

    /* image / pdf */
    .media-body {
        flex: 1;
        min-height: 0;
        overflow: auto;
        text-align: center;
        background: #101216;
        border-radius: var(--radius-sm, 6px);
    }

    .preview-img {
        max-width: 100%;
        max-height: 100%;
        object-fit: contain;
        display: inline-block;
        vertical-align: middle;
    }

    .preview-frame {
        flex: 1;
        min-height: 0;
        width: 100%;
        border: none;
        border-radius: var(--radius-sm, 6px);
        background: #fff;
    }

    /* markdown */
    .md-toolbar {
        display: flex;
        gap: 8px;
        margin-bottom: 8px;
        flex: none;
    }

    .md-find {
        flex: 1;
        max-width: 260px;
        padding: 4px 8px;
        font-size: 12px;
        border: 1px solid var(--border, #444);
        border-radius: 6px;
        background: var(--bg, #1a1c1e);
        color: var(--text);
    }

    .md-body {
        flex: 1;
        min-height: 0;
        overflow: auto;
        padding: 4px 8px 16px;
        font-size: 13px;
        line-height: 1.6;
        color: var(--text);
        background: var(--bg, #1a1c1e);
        border-radius: var(--radius-sm, 6px);
    }
    .md-body :global(h1), .md-body :global(h2), .md-body :global(h3) {
        margin: 0.6em 0 0.3em;
        line-height: 1.3;
    }
    .md-body :global(pre) {
        background: color-mix(in srgb, var(--text) 8%, transparent);
        padding: 8px 10px;
        border-radius: 6px;
        overflow: auto;
        font-size: 12px;
    }
    .md-body :global(code) {
        font-family: var(--term-font, Consolas, monospace);
    }
    .md-body :global(table) {
        border-collapse: collapse;
        margin: 0.5em 0;
    }
    .md-body :global(th), .md-body :global(td) {
        border: 1px solid var(--border, #444);
        padding: 3px 8px;
    }
    .md-body :global(img) { max-width: 100%; }

    /* csv / tsv */
    .table-toolbar {
        display: flex;
        align-items: center;
        gap: 10px;
        margin-bottom: 8px;
        flex: none;
    }

    .table-filter {
        flex: 1;
        max-width: 320px;
        padding: 4px 8px;
        font-size: 12px;
        border: 1px solid var(--border, #444);
        border-radius: 6px;
        background: var(--bg, #1a1c1e);
        color: var(--text);
    }

    .table-count {
        font-size: 11px;
        color: var(--text-secondary, #6b7280);
        white-space: nowrap;
    }

    .table-wrap {
        flex: 1;
        min-height: 0;
        overflow: auto;
        border: 1px solid var(--border, #444);
        border-radius: var(--radius-sm, 6px);
    }

    .data-table {
        border-collapse: collapse;
        font-size: 12px;
        min-width: 100%;
    }
    .data-table :global(th) {
        position: sticky;
        top: 0;
        background: color-mix(in srgb, var(--text) 10%, transparent);
        color: var(--text);
        font-weight: 600;
        text-align: left;
        padding: 4px 8px;
        white-space: nowrap;
        border-bottom: 1px solid var(--border, #444);
        z-index: 1;
    }
    .data-table :global(td) {
        padding: 3px 8px;
        border-bottom: 1px solid color-mix(in srgb, var(--border, #444) 50%, transparent);
        max-width: 320px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .data-table :global(tr):hover :global(td) {
        background: color-mix(in srgb, var(--text) 5%, transparent);
    }

    /* code */
    .code-wrap {
        flex: 1;
        min-height: 0;
        display: flex;
        flex-direction: column;
    }

    .cm-host {
        flex: 1;
        min-height: 0;
        overflow: hidden;
        border: 1px solid var(--border, #444);
        border-radius: var(--radius-sm, 6px);
    }

    .preview-note {
        margin: 6px 0 0;
        font-size: 11px;
        color: var(--text-secondary, #6b7280);
        flex: none;
    }

    .truncated-note {
        color: var(--warn, #c9a227);
    }
</style>
