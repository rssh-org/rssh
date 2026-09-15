<script lang="ts">
    // Remote file editor (VS Code style): opens a remote text/code file in a
    // full CodeMirror editor, edits locally, Ctrl+S saves back to the server
    // through the existing `sftp_upload` command. Replaces the SFTP list view
    // while open; back returns to the list (unsaved changes get a confirm).
    import { onDestroy, onMount } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import { EditorView, basicSetup } from "codemirror";
    import { EditorState } from "@codemirror/state";
    import { keymap } from "@codemirror/view";
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
    import { errMsg, t } from "../i18n/index.svelte.ts";
    import { toast } from "../stores/toast.svelte.ts";
    import Modal from "./Modal.svelte";
    import { cmLanguageFor, PREVIEW_CM_MAX } from "../sftp-preview.ts";

    let {
        name,
        sftpId,
        path,
        size,
        onClose,
        onSaved,
        onOpenWindow,
        onDirtyChange,
        onReveal,
        saveToken,
        active = true,
    }: {
        name: string;
        sftpId: string;
        path: string;
        size?: number;
        onClose: () => void;
        onSaved?: () => void;
        onOpenWindow?: () => void;
        onDirtyChange?: (dirty: boolean) => void;
        onReveal?: () => void;
        saveToken?: number;
        active?: boolean;
    } = $props();

    const MODES: Record<string, unknown> = {
        shell, python, r, perl, rust, go, javascript, clike,
        css, sql, xml, yaml, toml, properties, powerShell,
    };

    let loading = $state(true);
    let error = $state("");
    let dirty = $state(false);
    let saving = $state(false);
    let savedAt = $state("");
    let wasNonUtf8 = $state(false);
    let tooLarge = $state(false);
    // Save/discard/cancel confirm shown when leaving with unsaved changes.
    let confirmCloseOpen = $state(false);
    let cmEl = $state<HTMLDivElement | null>(null);
    let view: EditorView | null = null;
    let originalText = "";

    /** Report dirty to the host. Explicit calls only — never a $effect that
     *  tracks the `onDirtyChange` prop (parent re-renders pass a fresh arrow
     *  function, which made the effect re-run and could recurse). */
    function reportDirty(d: boolean) {
        if (d === dirty) return;
        dirty = d;
        onDirtyChange?.(d);
    }

    /** Bounded wait so a hung remote read surfaces as an error, not a spinner. */
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

    // Ignore late async results after the pane unmounted (tab closed / file switched).
    let loadAbort = false;

    function formatSize(n: number): string {
        if (n >= 1024 * 1024 * 1024) return (n / 1024 / 1024 / 1024).toFixed(1) + " GB";
        if (n >= 1024 * 1024) return (n / 1024 / 1024).toFixed(1) + " MB";
        if (n >= 1024) return (n / 1024).toFixed(1) + " KB";
        return n + " B";
    }

    /** Decode, remembering whether a non-UTF-8 fallback was needed. */
    function decode(bytes: Uint8Array): string {
        if (bytes.length >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) {
            return new TextDecoder("utf-8").decode(bytes.subarray(3));
        }
        try {
            return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
        } catch {
            wasNonUtf8 = true;
            try { return new TextDecoder("gb18030").decode(bytes); }
            catch { return new TextDecoder("utf-8").decode(bytes); }
        }
    }

    async function load() {
        if (loadAbort) return;
        loading = true;
        error = "";
        void invoke("log_frontend_error", { msg: `[event] load start ${path}` }).catch(() => {});
        if (size !== undefined && size > PREVIEW_CM_MAX) {
            error = t("sftp.editor.too_large", { size: formatSize(PREVIEW_CM_MAX) });
            loading = false;
            return;
        }
        try {
            // Slow remote storage (e.g. the 96%-full /lshome NFS mount) can
            // hang a read forever with no error — bound the wait so the user
            // can retry or leave instead of staring at "加载中…".
            const arr = await withTimeout(invoke<number[]>("sftp_download", { sftpId, path }), 30_000);
            if (loadAbort) return;
            void invoke("log_frontend_error", { msg: `[event] load got ${arr.length} bytes` }).catch(() => {});
            const bytes = new Uint8Array(arr.length);
            for (let i = 0; i < arr.length; i++) bytes[i] = arr[i];
            if (bytes.length > PREVIEW_CM_MAX) {
                error = t("sftp.editor.too_large", { size: formatSize(PREVIEW_CM_MAX) });
                loading = false;
                return;
            }
            originalText = decode(bytes);
        } catch (e: any) {
            if (loadAbort) return;
            const msg = e && e.message === "rssh-load-timeout"
                ? t("sftp.editor.load_timeout")
                : t("sftp.editor.load_failed", { err: errMsg(e) });
            void invoke("log_frontend_error", { msg: `[event] load failed: ${msg}` }).catch(() => {});
            error = msg;
        }
        loading = false;
    }

    function retry() {
        void load();
    }

    // Mount the editable editor once content is available.
    $effect(() => {
        if (loading || !cmEl || view || error) return;
        const mode = cmLanguageFor(name);
        const lang = MODES[mode];
        const extensions: any[] = [
            basicSetup,
            oneDark,
            EditorView.lineWrapping,
            EditorView.theme({
                "&": { height: "100%", fontSize: "12.5px" },
                ".cm-scroller": { fontFamily: "var(--term-font, Consolas, 'Cascadia Mono', monospace)", lineHeight: "1.45" },
                ".cm-gutters": { backgroundColor: "transparent" },
            }),
            keymap.of([{
                key: "Mod-s",
                run: () => { void save(); return true; },
            }]),
            EditorView.updateListener.of((u) => {
                if (u.docChanged) reportDirty(true);
            }),
        ];
        if (lang) extensions.push(StreamLanguage.define(lang as any));
        try {
            view = new EditorView({
                state: EditorState.create({ doc: originalText, extensions }),
                parent: cmEl,
            });
            view.focus();
        } catch (e: any) {
            // A CodeMirror init failure must surface as an error, never take
            // down the whole window (the previous behaviour froze the UI).
            error = t("sftp.editor.view_failed", { err: errMsg(e) });
            void invoke("log_frontend_error", { msg: `EditorView init failed: ${errMsg(e)}` }).catch(() => {});
        }
    });

    onMount(() => {
        void load();
        // Initial dirty report (clean) so the host has a baseline for this tab.
        reportDirty(false);
    });

    // The pane stays mounted while hidden (tab switch toggles visibility), so
    // re-measure and focus when it becomes visible again (mirrors EditPane).
    $effect(() => {
        if (active && view) {
            view.requestMeasure();
            view.focus();
        }
    });

    onDestroy(() => { loadAbort = true; view?.destroy(); view = null; });

    async function save(): Promise<boolean> {
        if (!view || saving) return false;
        saving = true;
        error = "";
        try {
            const text = view.state.doc.toString();
            const bytes = new TextEncoder().encode(text);
            await invoke("sftp_upload", { sftpId, path, data: Array.from(bytes) });
            originalText = text;
            reportDirty(false);
            const now = new Date();
            savedAt = now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
            toast.success(t("sftp.editor.saved_ok"));
            onSaved?.();
            return true;
        } catch (e: any) {
            error = t("sftp.editor.save_failed", { err: errMsg(e) });
            return false;
        } finally {
            saving = false;
        }
    }

    /** Leaving with unsaved changes asks save / discard / cancel instead of
     *  dropping the edit silently. */
    function requestClose() {
        if (dirty) {
            confirmCloseOpen = true;
            return;
        }
        onClose();
    }

    async function saveAndLeave() {
        if (await save()) {
            confirmCloseOpen = false;
            onClose();
        }
        // Save failed: keep the confirm open so the user can retry or cancel.
    }

    function discardAndLeave() {
        confirmCloseOpen = false;
        onClose();
    }

    function onKey(e: KeyboardEvent) {
        if (e.key === "Escape") { e.preventDefault(); requestClose(); }
    }
    onMount(() => {
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    });

    /** Ctrl+click on the filename asks the SFTP tree to reveal this file. */
    function onNameClick(e: MouseEvent) {
        if ((e.ctrlKey || e.metaKey) && onReveal) {
            e.preventDefault();
            onReveal();
        }
    }

    // Host-driven save: the parent bumps `saveToken` (e.g. AppShell's tab-close
    // "save and leave") and we save once per bump. Skips the initial value.
    let lastSaveToken = $state(0);
    $effect(() => {
        if (saveToken && saveToken !== lastSaveToken) {
            lastSaveToken = saveToken;
            void save();
        }
    });
</script>

<div class="editor-pane">
    <div class="editor-toolbar">
        <button type="button" class="btn btn-sm" onclick={requestClose} title={t("sftp.editor.back")}>← {t("sftp.editor.back")}</button>
        <span class="editor-name" title={path + (onReveal ? " — " + t("sftp.editor.reveal_hint") : "")}
            class:revealable={!!onReveal} onclick={onNameClick}>{name}{dirty ? " *" : ""}</span>
        <span class="editor-path" title={path}>{path}</span>
        <span class="editor-status">
            {#if saving}
                {t("sftp.editor.saving")}
            {:else if error}
                <span class="editor-status-error">{t("sftp.editor.save_failed_short")}</span>
            {:else if dirty}
                {t("sftp.editor.dirty")}
            {:else if savedAt}
                {t("sftp.editor.saved_at", { time: savedAt })}
            {:else}
                {t("sftp.editor.ready")}
            {/if}
        </span>
        <div class="editor-actions">
            {#if onOpenWindow}
                <button type="button" class="btn-icon" onclick={onOpenWindow}
                    title={t("sftp.ctx.open_window")} aria-label={t("sftp.ctx.open_window")}>▣</button>
            {/if}
            <button type="button" class="btn btn-sm" class:btn-dirty={dirty} onclick={() => void save()} disabled={saving || !dirty}>
                {saving ? t("sftp.editor.saving") : t("sftp.editor.save")} <span class="save-kbd">Ctrl+S</span>
            </button>
        </div>
    </div>

    {#if wasNonUtf8}
        <p class="editor-note">{t("sftp.editor.non_utf8")}</p>
    {/if}

    {#if loading}
        <p class="loading">{t("common.loading")}</p>
    {:else if error}
        <div class="editor-error">
            <p>{error}</p>
            <div class="editor-error-actions">
                <button type="button" class="btn btn-sm" onclick={retry}>{t("sftp.editor.retry")}</button>
                <button type="button" class="btn btn-sm" onclick={requestClose}>{t("sftp.editor.back")}</button>
            </div>
        </div>
    {:else}
        <div class="editor-body">
            <div class="cm-host" bind:this={cmEl}></div>
            <p class="editor-hint">{t("sftp.editor.hint")}</p>
        </div>
    {/if}

    {#if confirmCloseOpen}
        <Modal onClose={() => (confirmCloseOpen = false)} style="min-width: 360px;">
            <p class="confirm-title">{t("sftp.editor.unsaved_title")}</p>
            <p class="confirm-text">{t("sftp.editor.unsaved_body", { name })}</p>
            <div class="confirm-actions">
                <button type="button" class="btn btn-sm" class:btn-dirty={true} disabled={saving} onclick={() => void saveAndLeave()}>
                    {saving ? t("sftp.editor.saving") : t("sftp.editor.save_and_leave")}
                </button>
                <button type="button" class="btn btn-sm" onclick={discardAndLeave}>{t("sftp.editor.discard")}</button>
                <button type="button" class="btn btn-sm" onclick={() => (confirmCloseOpen = false)}>{t("common.cancel")}</button>
            </div>
        </Modal>
    {/if}
</div>

<style>
    .editor-pane {
        display: flex;
        flex-direction: column;
        height: 100%;
        min-height: 0;
        padding: 10px 14px;
        box-sizing: border-box;
    }

    .editor-toolbar {
        display: flex;
        align-items: center;
        gap: 10px;
        margin-bottom: 8px;
        flex: none;
        flex-wrap: wrap;
    }

    .editor-name {
        font-size: 13px;
        font-weight: 600;
        color: var(--text);
        white-space: nowrap;
    }

    .editor-name.revealable {
        cursor: pointer;
        text-decoration: underline dotted;
        text-underline-offset: 3px;
    }
    .editor-name.revealable:hover { color: var(--accent, #8bc8ea); }

    .editor-path {
        font-size: 11px;
        color: var(--text-secondary, #6b7280);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        min-width: 0;
        flex: 1;
    }

    .editor-status {
        font-size: 11px;
        color: var(--text-secondary, #6b7280);
        white-space: nowrap;
        flex: none;
    }

    .editor-status-error { color: var(--danger, #d64545); }

    .editor-actions {
        display: flex;
        align-items: center;
        gap: 6px;
        flex: none;
    }

    .save-kbd {
        font-size: 10px;
        opacity: 0.7;
        border: 1px solid currentColor;
        border-radius: 3px;
        padding: 0 3px;
        margin-left: 2px;
    }

    .btn-dirty {
        border-color: var(--accent, #8bc8ea) !important;
        color: var(--accent, #8bc8ea) !important;
    }

    .editor-note {
        margin: 0 0 6px;
        font-size: 11px;
        color: var(--warn, #c9a227);
        flex: none;
    }

    .loading { color: var(--text-secondary, #6b7280); }

    .editor-error {
        flex: 1;
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 10px;
        color: var(--danger, #d64545);
        font-size: 13px;
        padding-top: 16px;
    }

    .editor-error-actions {
        display: flex;
        gap: 8px;
    }

    .editor-body {
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

    .editor-hint {
        margin: 6px 0 0;
        font-size: 11px;
        color: var(--text-secondary, #6b7280);
        flex: none;
    }

    .confirm-title {
        margin: 0 0 8px;
        font-size: 14px;
        font-weight: 600;
        color: var(--text);
    }

    .confirm-text {
        margin: 0 0 16px;
        font-size: 13px;
        color: var(--text-secondary, #9aa0a6);
        word-break: break-all;
    }

    .confirm-actions {
        display: flex;
        gap: 8px;
        justify-content: flex-end;
    }
</style>
