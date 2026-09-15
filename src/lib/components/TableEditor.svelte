<script lang="ts">
    // Editable CSV/TSV grid (Excel style). Parses the remote delimited file,
    // renders every cell as an input, and Ctrl+S serializes the grid back to
    // text and writes it to the server via `sftp_upload`. Large grids fall
    // back to plain-text editing.
    import { onDestroy, onMount } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import { errMsg, t } from "../i18n/index.svelte.ts";
    import { toast } from "../stores/toast.svelte.ts";
    import Modal from "./Modal.svelte";
    import {
        parseDelimited,
        serializeDelimited,
        PREVIEW_TABLE_MAX_ROWS,
        PREVIEW_TABLE_MAX_COLS,
    } from "../sftp-preview.ts";

    // Above this many cells the grid editor would be unusably slow.
    const EDIT_TABLE_MAX_CELLS = 20_000;

    let {
        name,
        sftpId,
        path,
        size,
        onClose,
        onSwitchText,
        onDirtyChange,
        saveToken,
        active = true,
    }: {
        name: string;
        sftpId: string;
        path: string;
        size?: number;
        onClose: () => void;
        onSwitchText: () => void;
        onDirtyChange?: (dirty: boolean) => void;
        saveToken?: number;
        active?: boolean;
    } = $props();

    const delim = name.toLowerCase().endsWith(".tsv") ? "\t" : ",";

    let loading = $state(true);
    let error = $state("");
    let dirty = $state(false);
    let saving = $state(false);
    let savedAt = $state("");
    let wasNonUtf8 = $state(false);
    let hadBom = $state(false);
    let tooLarge = $state(false);
    let rows = $state<string[][]>([]);
    let totalCells = $state(0);
    // Frozen header + sorting: the first row is the header; body rows keep a
    // stable order array so sorting never corrupts edits, and save writes the
    // current (sorted) order back to the server.
    let sortCol = $state<number | null>(null);
    let sortDir = $state<"asc" | "desc">("asc");
    let order = $state<number[]>([]);
    // Save/discard/cancel confirm shown when leaving with unsaved changes.
    let confirmCloseOpen = $state(false);
    // Pending action after the confirm resolves (close, or switch to text mode).
    let pendingAction = $state<"close" | "switchText" | null>(null);

    // Report dirty to the host. Explicit calls only — never a $effect that
    // tracks the `onDirtyChange` prop (parent re-renders pass a fresh arrow
    // function, which made the effect re-run and could recurse).
    function reportDirty(d: boolean) {
        if (d === dirty) return;
        dirty = d;
        onDirtyChange?.(d);
    }

    const colCount = $derived(rows.reduce((m, r) => Math.max(m, r.length), 0));
    const rowCount = $derived(rows.length);
    const header = $derived(rows[0] ?? []);
    const bodyRows = $derived(order.map((i) => rows[i]));

    /** Numeric-aware comparator for a column. */
    function compareCells(a: string, b: string): number {
        const na = a !== "" && !Number.isNaN(Number(a));
        const nb = b !== "" && !Number.isNaN(Number(b));
        if (na && nb) return Number(a) - Number(b);
        return a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" });
    }

    function sortBy(col: number) {
        if (sortCol === col) {
            sortDir = sortDir === "asc" ? "desc" : "asc";
        } else {
            sortCol = col;
            sortDir = "asc";
        }
        const dir = sortDir === "asc" ? 1 : -1;
        order = order.slice().sort((a, b) => dir * compareCells(rows[a][col] ?? "", rows[b][col] ?? ""));
        reportDirty(true);
    }

    function decode(bytes: Uint8Array): string {
        if (bytes.length >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) {
            hadBom = true;
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

    // Ignore late async results after the pane unmounted.
    let loadAbort = false;

    async function load() {
        if (loadAbort) return;
        loading = true;
        error = "";
        void invoke("log_frontend_error", { msg: `[event] table load start ${path}` }).catch(() => {});
        try {
            // Slow remote storage can hang a read forever — bound the wait so
            // the user can retry or leave instead of staring at "加载中…".
            const arr = await withTimeout(invoke<number[]>("sftp_download", { sftpId, path }), 30_000);
            if (loadAbort) return;
            void invoke("log_frontend_error", { msg: `[event] table load got ${arr.length} bytes` }).catch(() => {});
            const bytes = new Uint8Array(arr.length);
            for (let i = 0; i < arr.length; i++) bytes[i] = arr[i];
            const text = decode(bytes);
            const parsed = parseDelimited(text, delim);
            const maxCols = parsed.reduce((m, r) => Math.max(m, r.length), 0);
            const cells = parsed.length * maxCols;
            if (parsed.length > PREVIEW_TABLE_MAX_ROWS || maxCols > PREVIEW_TABLE_MAX_COLS || cells > EDIT_TABLE_MAX_CELLS) {
                tooLarge = true;
                totalCells = cells;
            } else {
                rows = parsed;
                totalCells = cells;
                order = parsed.slice(1).map((_, i) => i + 1);
            }
        } catch (e: any) {
            if (loadAbort) return;
            const msg = e && e.message === "rssh-load-timeout"
                ? t("sftp.editor.load_timeout")
                : t("sftp.editor.load_failed", { err: errMsg(e) });
            void invoke("log_frontend_error", { msg: `[event] table load failed: ${msg}` }).catch(() => {});
            error = msg;
        }
        loading = false;
    }

    function retry() {
        void load();
    }

    onMount(() => {
        void load();
        // Initial dirty report (clean) so the host has a baseline for this tab.
        reportDirty(false);
    });

    function onCell(ri: number, ci: number, value: string) {
        const row = rows[order[ri]];
        if (!row) return;
        while (row.length <= ci) row.push("");
        row[ci] = value;
        reportDirty(true);
    }

    async function save(): Promise<boolean> {
        if (saving) return false;
        saving = true;
        error = "";
        try {
            const text = serializeDelimited(rows, delim);
            const bytes = new TextEncoder().encode(hadBom ? "\uFEFF" + text : text);
            await invoke("sftp_upload", { sftpId, path, data: Array.from(bytes) });
            reportDirty(false);
            const now = new Date();
            savedAt = now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
            toast.success(t("sftp.editor.saved_ok"));
            return true;
        } catch (e: any) {
            error = t("sftp.editor.save_failed", { err: errMsg(e) });
            return false;
        } finally {
            saving = false;
        }
    }

    /** Leaving with unsaved changes asks save / discard / cancel. */
    function requestClose() {
        if (dirty) {
            pendingAction = "close";
            confirmCloseOpen = true;
            return;
        }
        onClose();
    }

    /** Switching to text mode with unsaved changes asks first too. */
    function requestSwitchText() {
        if (dirty) {
            pendingAction = "switchText";
            confirmCloseOpen = true;
            return;
        }
        onSwitchText();
    }

    async function saveAndContinue() {
        if (await save()) {
            confirmCloseOpen = false;
            const action = pendingAction;
            pendingAction = null;
            if (action === "close") onClose();
            else if (action === "switchText") onSwitchText();
        }
        // Save failed: keep the confirm open so the user can retry or cancel.
    }

    function discardAndContinue() {
        confirmCloseOpen = false;
        const action = pendingAction;
        pendingAction = null;
        if (action === "close") onClose();
        else if (action === "switchText") onSwitchText();
    }

    onMount(() => {
        function onKey(e: KeyboardEvent) {
            if (e.key === "Escape") { e.preventDefault(); requestClose(); }
        }
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    });

    onDestroy(() => { loadAbort = true; });

    // Host-driven save: the parent bumps `saveToken` and we save once per bump.
    let lastSaveToken = $state(0);
    $effect(() => {
        if (saveToken && saveToken !== lastSaveToken) {
            lastSaveToken = saveToken;
            void save();
        }
    });
</script>

<div class="table-editor">
    <div class="table-toolbar">
        <button type="button" class="btn btn-sm" onclick={requestClose} title={t("sftp.editor.back")}>← {t("sftp.editor.back")}</button>
        <span class="table-name" title={path}>{name}{dirty ? " *" : ""}</span>
        <span class="table-dims">{t("sftp.table.rows_cols", { rows: rowCount, cols: colCount })}</span>
        <span class="table-status">
            {#if saving}
                {t("sftp.editor.saving")}
            {:else if error}
                <span class="table-status-error">{t("sftp.editor.save_failed_short")}</span>
            {:else if dirty}
                {t("sftp.editor.dirty")}
            {:else if savedAt}
                {t("sftp.editor.saved_at", { time: savedAt })}
            {:else}
                {t("sftp.editor.ready")}
            {/if}
        </span>
        <div class="table-actions">
            <button type="button" class="btn btn-sm" onclick={requestSwitchText} disabled={saving}>{t("sftp.table.text_mode")}</button>
            <button type="button" class="btn btn-sm" class:btn-dirty={dirty} onclick={() => void save()} disabled={saving || !dirty}>
                {saving ? t("sftp.editor.saving") : t("sftp.editor.save")} <span class="save-kbd">Ctrl+S</span>
            </button>
        </div>
    </div>

    {#if wasNonUtf8}
        <p class="table-note">{t("sftp.editor.non_utf8")}</p>
    {/if}

    {#if loading}
        <p class="loading">{t("common.loading")}</p>
    {:else if error}
        <div class="table-error">
            <p>{error}</p>
            <div class="table-error-actions">
                <button type="button" class="btn btn-sm" onclick={retry}>{t("sftp.editor.retry")}</button>
                <button type="button" class="btn btn-sm" onclick={requestClose}>{t("sftp.editor.back")}</button>
            </div>
        </div>
    {:else if tooLarge}
        <div class="table-error">
            <p>{t("sftp.table.too_large", { cells: totalCells.toLocaleString() })}</p>
            <button type="button" class="btn btn-sm" onclick={switchText}>{t("sftp.table.text_mode")}</button>
        </div>
    {:else}
        <div class="table-scroll">
            <table class="grid">
                <thead>
                    <tr>
                        <th class="rowhead">#</th>
                        {#each Array(colCount) as _, ci}
                            <th class="colhead" class:sorted={sortCol === ci} onclick={() => sortBy(ci)} title={t("sftp.table.sort_hint")}>
                                <span class="head-cell">
                                    <span class="head-text">{header[ci] ?? `C${ci + 1}`}</span>
                                    {#if sortCol === ci}
                                        <span class="sort-mark">{sortDir === "asc" ? "▲" : "▼"}</span>
                                    {/if}
                                </span>
                            </th>
                        {/each}
                    </tr>
                </thead>
                <tbody>
                    {#each bodyRows as row, ri (order[ri])}
                        <tr>
                            <td class="rowhead">{order[ri] + 1}</td>
                            {#each Array(colCount) as _, ci}
                                <td>
                                    <input
                                        type="text"
                                        value={row[ci] ?? ""}
                                        oninput={(e) => onCell(ri, ci, (e.currentTarget as HTMLInputElement).value)}
                                        aria-label={`r${order[ri] + 1}c${ci + 1}`}
                                    />
                                </td>
                            {/each}
                        </tr>
                    {/each}
                </tbody>
            </table>
        </div>
        <p class="table-hint">{t("sftp.table.hint")}</p>
    {/if}

    {#if confirmCloseOpen}
        <Modal onClose={() => { confirmCloseOpen = false; pendingAction = null; }} style="min-width: 360px;">
            <p class="confirm-title">{t("sftp.editor.unsaved_title")}</p>
            <p class="confirm-text">{t("sftp.editor.unsaved_body", { name })}</p>
            <div class="confirm-actions">
                <button type="button" class="btn btn-sm" class:btn-dirty={true} disabled={saving} onclick={() => void saveAndContinue()}>
                    {saving ? t("sftp.editor.saving") : (pendingAction === "switchText" ? t("sftp.editor.save_and_switch") : t("sftp.editor.save_and_leave"))}
                </button>
                <button type="button" class="btn btn-sm" onclick={discardAndContinue}>{t("sftp.editor.discard")}</button>
                <button type="button" class="btn btn-sm" onclick={() => { confirmCloseOpen = false; pendingAction = null; }}>{t("common.cancel")}</button>
            </div>
        </Modal>
    {/if}
</div>

<style>
    .table-editor {
        display: flex;
        flex-direction: column;
        height: 100%;
        min-height: 0;
        padding: 10px 14px;
        box-sizing: border-box;
    }

    .table-toolbar {
        display: flex;
        align-items: center;
        gap: 10px;
        margin-bottom: 8px;
        flex: none;
        flex-wrap: wrap;
    }

    .table-name {
        font-size: 13px;
        font-weight: 600;
        color: var(--text);
        white-space: nowrap;
    }

    .table-dims {
        font-size: 11px;
        color: var(--text-secondary, #6b7280);
        white-space: nowrap;
    }

    .table-status {
        font-size: 11px;
        color: var(--text-secondary, #6b7280);
        white-space: nowrap;
        flex: none;
    }

    .table-status-error { color: var(--danger, #d64545); }

    .table-actions {
        display: flex;
        align-items: center;
        gap: 6px;
        flex: none;
        margin-left: auto;
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

    .table-note {
        margin: 0 0 6px;
        font-size: 11px;
        color: var(--warn, #c9a227);
        flex: none;
    }

    .loading { color: var(--text-secondary, #6b7280); }

    .table-error {
        flex: 1;
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 10px;
        color: var(--danger, #d64545);
        font-size: 13px;
        padding-top: 16px;
    }

    .table-error-actions {
        display: flex;
        gap: 8px;
    }

    .table-scroll {
        flex: 1;
        min-height: 0;
        overflow: auto;
        border: 1px solid var(--border, #444);
        border-radius: var(--radius-sm, 6px);
    }

    .grid {
        border-collapse: separate;
        border-spacing: 0;
        min-width: 100%;
        font-size: 12px;
    }

    .grid th,
    .grid td {
        border-right: 1px solid var(--border, #2a2d33);
        border-bottom: 1px solid var(--border, #2a2d33);
        padding: 0;
    }

    /* Frozen header row */
    .grid thead th {
        position: sticky;
        top: 0;
        z-index: 3;
        background: var(--bg-3, #23262b);
        cursor: pointer;
        user-select: none;
        white-space: nowrap;
        padding: 0 6px;
        height: 26px;
    }

    .grid thead th:hover { background: var(--bg-4, #2c3036); }

    .grid thead th.sorted { color: var(--accent, #8bc8ea); }

    .head-cell {
        display: inline-flex;
        align-items: center;
        gap: 4px;
        font-weight: 600;
        color: inherit;
    }

    .sort-mark { font-size: 9px; }

    /* Frozen row-number column */
    .grid td.rowhead,
    .grid th.rowhead {
        position: sticky;
        left: 0;
        z-index: 2;
        min-width: 44px;
        width: 44px;
        text-align: right;
        padding: 3px 8px;
        color: var(--text-secondary, #6b7280);
        font-size: 11px;
        background: var(--bg-3, #23262b);
    }

    .grid thead th.rowhead { z-index: 4; }

    .grid input {
        width: 120px;
        min-width: 120px;
        border: none;
        background: transparent;
        color: var(--text);
        font: inherit;
        padding: 3px 6px;
        box-sizing: border-box;
        outline: none;
    }

    .grid input:focus {
        background: var(--accent-soft, rgba(139, 200, 234, 0.15));
        outline: 1px solid var(--accent, #8bc8ea);
    }

    .table-hint {
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
