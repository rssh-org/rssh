<script lang="ts">
    import {onDestroy, onMount, tick} from "svelte";
    import {invoke} from "@tauri-apps/api/core";
    import {WebviewWindow} from "@tauri-apps/api/webviewWindow";
    import * as app from "../stores/app.svelte.ts";
    import * as transfers from "../stores/transfers.svelte.ts";
    import type {RemoteEntry} from "../stores/app.svelte.ts";
    import { errMsg, t } from "../i18n/index.svelte.ts";
    import { fileStamp } from "../save-file.ts";
    import { remoteUploadName } from "../sftp-name.ts";
    import Modal from "./Modal.svelte";
    import AppIcon from "./AppIcon.svelte";
    import PreviewPane from "./PreviewPane.svelte";
    import { previewKind } from "../sftp-preview.ts";
    import {writeText as writeClipboard} from "../clipboard.ts";
    import {toast} from "../stores/toast.svelte.ts";

    /** Mirrors the backend WalkEntry; rel_path is always '/'-separated. */
    interface WalkEntry { rel_path: string; size: number; }

    /** Mirrors the backend FileStat. */
    interface FileStat {
        name: string;
        is_dir: boolean;
        size: number;
        mtime: number;
        uid?: number;
        gid?: number;
        user?: string;
        group?: string;
        permissions?: number;
    }

    let {meta}: { meta: Record<string, string> } = $props();

    let sftpId = $state<string | null>(null);
    let cwd = $state("/");
    let home = $state("/");
    let pathInput = $state("/");
    let entries = $state<RemoteEntry[]>([]);
    let loading = $state(true);
    let error = $state("");
    let notice = $state("");

    /** Names of selected entries in the current directory. Cleared on
     *  directory change — selections do not persist across directories. */
    let selected = $state(new Set<string>());
    /** Open/close state of the Upload dropdown menu. */
    let uploadMenuOpen = $state(false);
    let uploadWrapEl: HTMLDivElement | undefined;
    /** "Select all" checkbox — bound so we can drive `indeterminate` from an
     *  effect; the attribute form does not reliably sync the DOM property. */
    let selectAllEl: HTMLInputElement | undefined;

    /** Context menu state. */
    let ctxMenu = $state<{ x: number; y: number; entry: RemoteEntry } | null>(null);
    let ctxMenuEl: HTMLDivElement | undefined;
    let ctxDx = $state(0);
    let ctxDy = $state(0);
    let ctxReady = $state(false);

    /** Properties dialog state. */
    let propsStat = $state<FileStat | null>(null);
    let propsLoading = $state(false);

    /** Rename dialog state. */
    let renameEntry = $state<RemoteEntry | null>(null);
    let renameValue = $state("");

    /** Delete confirm state. */
    let deleteEntry = $state<RemoteEntry | null>(null);

    /** Preview state: a remote file opened in the in-window modal. */
    let previewEntry = $state<RemoteEntry | null>(null);

    // "Reveal this file" from the editor tab: navigate to its directory and
    // briefly highlight the row. Only the SftpBrowser instance owning the same
    // SFTP session acts on the request.
    let pendingReveal = $state<{ name: string; nonce: number } | null>(null);
    let revealed = $state<string | null>(null);
    let revealTimer: number | undefined;

    // Files that are open in a tab (editor or preview) stay highlighted in the
    // tree, so the user can find "which figure did I just open" at a glance
    // among many similarly-named files — like VS Code's explorer.
    let openedPaths = $derived(
        new Set(
            app.workspaceTabs()
                .filter((t) => t.type === "sftp_edit" || t.type === "sftp_preview")
                .map((t) => t.meta?.path ?? ""),
        ),
    );
    let activeOpenPath = $derived(
        (() => {
            const id = app.activeWorkspaceId();
            const tab = app.workspaceTabs().find((t) => t.id === id);
            return tab && (tab.type === "sftp_edit" || tab.type === "sftp_preview")
                ? (tab.meta?.path ?? "")
                : "";
        })(),
    );

    $effect(() => {
        const req = app.revealRequest();
        if (!req || req.sftpId !== sftpId) return;
        const i = req.path.lastIndexOf("/");
        const dir = i > 0 ? req.path.slice(0, i) : "/";
        const name = req.path.slice(i + 1);
        pendingReveal = { name, nonce: req.nonce };
        void listDir(dir);
    });

    onDestroy(() => { clearTimeout(revealTimer); });

    /** Renames the input after mount so it can receive focus. */
    let renameInputEl: HTMLInputElement | undefined;

    const selectedCount = $derived(selected.size);
    const allSelected = $derived(entries.length > 0 && selected.size === entries.length);
    const someSelected = $derived(selected.size > 0 && selected.size < entries.length);

    // ── Sort: name / size / mtime, asc / desc. "" keeps the server order.
    //    Directories always sort before files (VS Code explorer style).
    type SortField = "name" | "size" | "mtime" | "";
    let sortBy = $state<SortField>("");
    let sortDir = $state<"asc" | "desc">("asc");
    function cmpFor(a: RemoteEntry, b: RemoteEntry): number {
        let r: number;
        if (sortBy === "name") {
            r = a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
        } else if (sortBy === "size") {
            r = (a.is_dir ? 0 : a.size) - (b.is_dir ? 0 : b.size);
        } else {
            r = a.mtime - b.mtime;
        }
        return sortDir === "asc" ? r : -r;
    }
    let sortedEntries = $derived.by(() => {
        if (!sortBy) return entries;
        const dirs = entries.filter((e) => e.is_dir).slice().sort(cmpFor);
        const files = entries.filter((e) => !e.is_dir).slice().sort(cmpFor);
        return [...dirs, ...files];
    });
    function toggleSort(field: SortField) {
        if (sortBy === field) {
            sortDir = sortDir === "asc" ? "desc" : "asc";
        } else {
            sortBy = field;
            sortDir = "asc";
        }
    }
    function sortArrow(field: SortField): string {
        return sortBy === field ? (sortDir === "asc" ? "▲" : "▼") : "";
    }

    onMount(async () => {
        try {
            let id: string;
            if (meta.sessionId) {
                // Reuse existing SSH connection — no re-authentication needed
                id = await invoke<string>("sftp_connect_session", { sessionId: meta.sessionId });
            } else {
                id = await invoke<string>("sftp_connect", {
                    host: meta.host, port: Number(meta.port),
                    username: meta.username, authType: meta.authType, secret: meta.secret || null,
                });
            }
            sftpId = id;
            const h = await invoke<string>("sftp_home", {sftpId: id});
            home = h;
            cwd = h;
            pathInput = h;
            await listDir(h);
        } catch (e: any) {
            error = errMsg(e);
            loading = false;
        }
    });

    onDestroy(() => {
        if (sftpId) invoke("sftp_close", {sftpId});
    });

    async function listDir(path: string) {
        loading = true;
        error = "";
        try {
            entries = await invoke<RemoteEntry[]>("sftp_list", {sftpId, path});
            // Clear selection on directory change — selection has no meaning across directories.
            selected = new Set();
            cwd = path;
            pathInput = path;
        } catch (e: any) {
            error = errMsg(e);
        }
        loading = false;
        // A pending "reveal file in tree" request (Ctrl+click in the editor
        // tab) finished loading: highlight and scroll to the file.
        if (pendingReveal) {
            const pr = pendingReveal;
            pendingReveal = null;
            if (entries.some((e) => e.name === pr.name)) {
                revealed = pr.name;
                clearTimeout(revealTimer);
                revealTimer = window.setTimeout(() => { revealed = null; }, 3000);
                requestAnimationFrame(() => {
                    document.querySelector(`[data-reveal="${CSS.escape(pr.name)}"]`)?.scrollIntoView({ block: "nearest" });
                });
            }
            app.clearRevealRequest(pr.nonce);
        }
    }

    function goUp() {
        const parent = cwd.replace(/\/[^/]+\/?$/, "") || "/";
        listDir(parent);
    }

    function expandHome(p: string): string {
        if (p !== "~" && !p.startsWith("~/")) return p;
        return (home + p.slice(1)).replace(/\/{2,}/g, "/");
    }

    function revertInput() {
        pathInput = cwd;
        error = "";
    }

    function submitPath() {
        const target = expandHome(pathInput.trim());
        if (!target) {
            revertInput();
            return;
        }
        listDir(target);
    }

    function onPathKeyDown(e: KeyboardEvent) {
        if (e.key === "Enter") {
            e.preventDefault();
            submitPath();
        } else if (e.key === "Escape") {
            revertInput();
            (e.currentTarget as HTMLInputElement).blur();
        }
    }

    function openEntry(e: RemoteEntry) {
        if (e.is_dir) { listDir(joinRemote(cwd, e.name)); return; }
        // Mobile has no select-and-download toolbar and can't rely on long-press,
        // so tapping a file is the download affordance. Desktop keeps the file
        // tap inert (it downloads via checkbox selection / context menu).
        if (app.isMobile) downloadEntry(e);
    }

    function basename(p: string): string {
        return p.split(/[\\/]/).pop() || p;
    }

    /** Join a remote path: always '/'-separated, empty segments filtered,
     *  root directory special-cased. */
    function joinRemote(...parts: string[]): string {
        let acc = "";
        for (const p of parts) {
            if (!p) continue;
            const cleaned = p.replace(/^\/+|\/+$/g, "");
            if (cleaned) acc += "/" + cleaned;
        }
        return acc || "/";
    }

    /** Join a local path: separator follows root (Windows '\\', Unix '/').
     *  '/' within rel_path is translated to the platform separator. */
    function joinLocal(root: string, ...rels: string[]): string {
        const sep = root.includes("\\") ? "\\" : "/";
        let acc = root.replace(/[\\/]+$/, "");
        for (const r of rels) {
            if (!r) continue;
            const cleaned = r.replace(/^[\\/]+|[\\/]+$/g, "").replace(/\//g, sep);
            if (cleaned) acc += sep + cleaned;
        }
        return acc;
    }

    function formatSize(bytes: number): string {
        if (bytes < 1024) return `${bytes} B`;
        if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} K`;
        if (bytes < 1073741824) return `${(bytes / 1048576).toFixed(1)} M`;
        return `${(bytes / 1073741824).toFixed(1)} G`;
    }

    /** Render mtime: current year as MM-DD HH:mm, otherwise YYYY-MM-DD.
     *  A value of 0 means the server did not provide an mtime. */
    function formatMtime(secs: number): string {
        if (!secs) return "—";
        const d = new Date(secs * 1000);
        const yy = d.getFullYear();
        const now = new Date();
        const pad = (n: number) => n.toString().padStart(2, "0");
        if (yy === now.getFullYear()) {
            return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
        }
        return `${yy}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
    }

    function formatPermissions(perm: number | undefined): string {
        if (perm === undefined || perm === null) return "—";
        const p = perm & 0o777;
        const rwx = (v: number) => (v & 4 ? "r" : "-") + (v & 2 ? "w" : "-") + (v & 1 ? "x" : "-");
        return rwx((p >> 6) & 7) + rwx((p >> 3) & 7) + rwx(p & 7);
    }

    function toggleSelected(name: string) {
        const next = new Set(selected);
        if (next.has(name)) next.delete(name);
        else next.add(name);
        selected = next;
    }

    function toggleAll() {
        if (selected.size === entries.length) selected = new Set();
        else selected = new Set(entries.map(e => e.name));
    }

    function closeUploadMenu() { uploadMenuOpen = false; }
    function toggleUploadMenu() { uploadMenuOpen = !uploadMenuOpen; }

    function onWindowMouseDown(ev: MouseEvent) {
        if (!uploadMenuOpen) return;
        const target = ev.target as Node | null;
        if (uploadWrapEl && target && !uploadWrapEl.contains(target)) closeUploadMenu();
    }

    $effect(() => {
        if (uploadMenuOpen) {
            window.addEventListener("mousedown", onWindowMouseDown);
            return () => window.removeEventListener("mousedown", onWindowMouseDown);
        }
    });

    // `indeterminate` is a DOM property only — Svelte's attribute spread does
    // not reliably set it. Sync it imperatively whenever the selection changes.
    $effect(() => {
        if (selectAllEl) selectAllEl.indeterminate = someSelected;
    });

    // ── Context menu ──

    function onContextMenu(e: MouseEvent, entry: RemoteEntry) {
        e.preventDefault();
        e.stopPropagation();
        if (selected.has(entry.name)) return;
        ctxDx = 0;
        ctxDy = 0;
        ctxReady = false;
        ctxMenu = { x: e.clientX, y: e.clientY, entry };
    }

    function closeCtxMenu() { ctxMenu = null; ctxReady = false; }

    function onSftpContextMenu(e: MouseEvent) {
        e.preventDefault();
    }

    function onCtxMenuMount() {
        if (!ctxMenuEl) return;
        const r = ctxMenuEl.getBoundingClientRect();
        if (r.right > window.innerWidth) ctxDx = window.innerWidth - r.right - 4;
        if (r.bottom > window.innerHeight) ctxDy = window.innerHeight - r.bottom - 4;
        ctxReady = true;
    }

    $effect(() => {
        if (ctxMenu && ctxMenuEl) onCtxMenuMount();
    });

    function onDocumentKeydown(e: KeyboardEvent) {
        if (e.key === "Escape" && ctxMenu) {
            e.preventDefault();
            e.stopPropagation();
            closeCtxMenu();
        }
    }

    onMount(() => {
        document.addEventListener("keydown", onDocumentKeydown);
    });

    onDestroy(() => {
        document.removeEventListener("keydown", onDocumentKeydown);
    });

    function entryPath(entry: RemoteEntry): string {
        return joinRemote(cwd, entry.name);
    }

    function copyPath(entry: RemoteEntry) {
        closeCtxMenu();
        void writeClipboard(entryPath(entry)).catch((error) => toast.error(errMsg(error)));
    }

    function copyPathToTerminal(entry: RemoteEntry) {
        // Remote filenames may legally contain control characters, including
        // newlines — sent raw to the shell, a hostile name like "x\nrm -rf ~\n"
        // would execute immediately. Strip C0 controls + DEL before sending.
        const path = entryPath(entry).replace(/[\x00-\x1f\x7f]/g, "");
        app.sendTextToActiveTerminal(path);
        closeCtxMenu();
    }

    /** Queue downloads for a set of entries into a local directory.
     *  Walks directories, queues individual files, and collects walk errors
     *  so the caller can display them. */
    async function queueDownloads(items: RemoteEntry[], dir: string): Promise<{ queued: number; walkErrors: string[] }> {
        let queued = 0;
        // Accumulate per-tree walk failures so users see every failed dir,
        // not just the last one.
        const walkErrors: string[] = [];
        for (const e of items) {
            const remote = joinRemote(cwd, e.name);
            if (e.is_dir) {
                // Expand each subtree into N independent Transfers. A walk
                // failure only skips that subtree; other selected entries
                // continue to be queued.
                try {
                    const walked = await invoke<WalkEntry[]>("sftp_walk_remote_dir", {
                        sftpId, remoteRoot: remote,
                    });
                    for (const w of walked) {
                        await transfers.startDownload({
                            sessionId: meta.sessionId,
                            remotePath: joinRemote(remote, w.rel_path),
                            localPath:  joinLocal(dir, e.name, w.rel_path),
                            sizeHint:   w.size,
                        });
                        queued++;
                    }
                } catch (err) {
                    walkErrors.push(`${e.name}: ${errMsg(err)}`);
                }
            } else {
                await transfers.startDownload({
                    sessionId: meta.sessionId,
                    remotePath: remote,
                    localPath:  joinLocal(dir, e.name),
                    sizeHint:   e.size,
                });
                queued++;
            }
        }
        return { queued, walkErrors };
    }

    async function downloadEntry(entry: RemoteEntry) {
        closeCtxMenu();
        error = "";
        notice = "";
        if (!meta.sessionId) { error = "Missing SSH session"; return; }
        try {
            if (entry.is_dir) {
                const dir = await invoke<string | null>("sftp_pick_folder");
                if (!dir) return;
                const { queued, walkErrors } = await queueDownloads([entry], dir);
                if (walkErrors.length > 0) error = `${t("sftp.walk_failed")}\n${walkErrors.join("\n")}`;
                if (queued > 0) notice = t("sftp.queued_n", { n: queued });
            } else if (app.isMobile) {
                // Mobile: pick a SAF save target via the dialog plugin and stream
                // to its content:// URI through the shared transfer queue.
                const { save } = await import("@tauri-apps/plugin-dialog");
                const target = await save({ defaultPath: entry.name });
                if (!target) return;
                await transfers.startDownload({
                    sessionId: meta.sessionId,
                    remotePath: entryPath(entry),
                    localPath:  target,
                    sizeHint:   entry.size,
                });
                notice = t("sftp.queued_n", { n: 1 });
            } else {
                const localPath = await invoke<string | null>("sftp_pick_save_path", {
                    defaultName: entry.name,
                });
                if (!localPath) return;
                await transfers.startDownload({
                    sessionId: meta.sessionId,
                    remotePath: entryPath(entry),
                    localPath,
                    sizeHint: entry.size,
                });
                notice = t("sftp.queued_n", { n: 1 });
            }
        } catch (err: any) {
            error = errMsg(err);
        }
    }

    function confirmDelete(entry: RemoteEntry) {
        closeCtxMenu();
        deleteEntry = entry;
    }

    async function doDelete() {
        if (!deleteEntry) return;
        const entry = deleteEntry;
        deleteEntry = null;
        error = "";
        try {
            await invoke("sftp_remove", { sftpId, path: entryPath(entry) });
            await listDir(cwd);
        } catch (e: any) {
            error = errMsg(e);
        }
    }

    function startRename(entry: RemoteEntry) {
        closeCtxMenu();
        renameEntry = entry;
        renameValue = entry.name;
        tick().then(() => { renameInputEl?.focus(); renameInputEl?.select(); });
    }

    async function doRename() {
        if (!renameEntry || !renameValue.trim()) return;
        if (renameValue.trim().includes("/")) return;
        const oldPath = entryPath(renameEntry);
        const newPath = joinRemote(cwd, renameValue.trim());
        error = "";
        try {
            await invoke("sftp_rename", { sftpId, oldPath, newPath });
            renameEntry = null;
            renameValue = "";
            await listDir(cwd);
        } catch (e: any) {
            error = errMsg(e);
        }
    }

    function showProperties(entry: RemoteEntry) {
        closeCtxMenu();
        propsLoading = true;
        propsStat = null;
        const path = entryPath(entry);
        invoke<FileStat>("sftp_stat", { sftpId, path })
            .then(stat => { if (propsLoading) { propsStat = stat; propsLoading = false; } })
            .catch((e: any) => { if (propsLoading) { error = errMsg(e); propsLoading = false; } });
    }

    function closeProperties() { propsStat = null; propsLoading = false; }

    // ── Preview (image / PDF / markdown / table / code) ──

    function isPreviewable(entry: RemoteEntry): boolean {
        if (entry.is_dir || entry.is_symlink) return false;
        return previewKind(entry.name) !== null;
    }

    /** Open the file in the in-window modal. The pane downloads and renders
     *  it itself (image/PDF/markdown/CSV·TSV/CodeMirror with Ctrl+F search). */
    function openPreview(entry: RemoteEntry) {
        closeCtxMenu();
        error = "";
        if (!isPreviewable(entry)) return;
        previewEntry = entry;
    }

    /** Open a preview-only file (image/PDF) in a main-window tab, so it shows
     *  in the tab bar like a VS Code editor tab. */
    function openPreviewTab(entry: RemoteEntry) {
        closeCtxMenu();
        error = "";
        if (!isPreviewable(entry)) return;
        app.openRemotePreview({
            sftpId: sftpId ?? "",
            path: entryPath(entry),
            name: entry.name,
            size: entry.size,
        });
    }

    /** Open the file in a standalone desktop window. The SFTP session belongs
     *  to the main window, so closing the preview window never disconnects. */
    async function openInWindow(entry: RemoteEntry) {
        closeCtxMenu();
        error = "";
        if (!isPreviewable(entry)) return;
        try {
            const q = new URLSearchParams({
                view: "preview",
                sftp: sftpId ?? "",
                path: entryPath(entry),
                name: entry.name,
                size: String(entry.size),
            });
            const win = new WebviewWindow("rssh-preview-" + Date.now(), {
                url: "index.html?" + q.toString(),
                title: entry.name,
                width: 1100,
                height: 760,
                minWidth: 480,
                minHeight: 360,
                resizable: true,
                // Same GPU workaround as the main window: WebView2's GPU
                // renderer can crash/flicker windows on machines with a flaky
                // GPU, so new preview windows get software rendering too.
                additionalBrowserArgs: "--disable-gpu",
            });
            win.once("tauri://error", (e: any) => {
                error = t("sftp.preview.window_failed", { err: String(e ?? "") });
            });
        } catch (e: any) {
            error = errMsg(e);
        }
    }

    // ── Edit (VS Code style) ──

    /** Text-ish files open in the editor; image/PDF stay preview-only. */
    function isEditable(entry: RemoteEntry): boolean {
        if (entry.is_dir || entry.is_symlink) return false;
        const k = previewKind(entry.name);
        return k === "code" || k === "markdown" || k === "table";
    }

    /** Open the file in a main-window editor tab (VS Code style): the SFTP
     *  sidebar stays mounted as the file tree, the editor opens in the center
     *  region. Reuses the tab when the same file is already open. */
    function openEditor(entry: RemoteEntry) {
        closeCtxMenu();
        error = "";
        if (!isEditable(entry)) return;
        app.openRemoteEdit({
            sftpId: sftpId ?? "",
            path: entryPath(entry),
            name: entry.name,
            size: entry.size,
        });
    }

    /** Open the file in a standalone edit window (multi-instance, VS Code tab
     *  style). CSV/TSV get the editable grid inside that window. */
    async function openEditorWindow(entry: RemoteEntry) {
        closeCtxMenu();
        error = "";
        if (!isEditable(entry)) return;
        try {
            const q = new URLSearchParams({
                view: "edit",
                sftp: sftpId ?? "",
                path: entryPath(entry),
                name: entry.name,
                size: String(entry.size),
            });
            const win = new WebviewWindow("rssh-edit-" + Date.now(), {
                url: "index.html?" + q.toString(),
                title: entry.name,
                width: 1100,
                height: 760,
                minWidth: 480,
                minHeight: 360,
                resizable: true,
                // Same GPU workaround as the main window: WebView2's GPU
                // renderer can crash/flicker windows on machines with a flaky
                // GPU, so new edit windows get software rendering too.
                additionalBrowserArgs: "--disable-gpu",
            });
            win.once("tauri://error", (e: any) => {
                error = t("sftp.preview.window_failed", { err: String(e ?? "") });
            });
        } catch (e: any) {
            error = errMsg(e);
        }
    }

    // ── Download selected ──

    async function downloadSelected() {
        error = "";
        notice = "";
        if (!meta.sessionId) { error = "Missing SSH session"; return; }
        if (selected.size === 0) return;
        const items = entries.filter(e => selected.has(e.name));
        try {
            const dir = await invoke<string | null>("sftp_pick_folder");
            if (!dir) return;
            const { queued, walkErrors } = await queueDownloads(items, dir);
            if (walkErrors.length > 0) error = `${t("sftp.walk_failed")}\n${walkErrors.join("\n")}`;
            if (queued > 0) notice = t("sftp.queued_n", { n: queued });
            selected = new Set();
        } catch (err: any) {
            error = errMsg(err);
        }
    }

    async function uploadFiles() {
        error = "";
        notice = "";
        if (!meta.sessionId) { error = "Missing SSH session"; return; }
        try {
            const paths = await invoke<string[] | null>("sftp_pick_open_files");
            if (!paths || paths.length === 0) return;
            for (const p of paths) {
                const name = basename(p);
                await transfers.startUpload({
                    sessionId: meta.sessionId,
                    localPath:  p,
                    remotePath: joinRemote(cwd, name),
                });
            }
            notice = t("sftp.queued_n", { n: paths.length });
        } catch (err: any) {
            error = errMsg(err);
        }
    }

    async function uploadFolder() {
        error = "";
        notice = "";
        if (!meta.sessionId) { error = "Missing SSH session"; return; }
        try {
            const dir = await invoke<string | null>("sftp_pick_folder");
            if (!dir) return;
            const walked = await invoke<WalkEntry[]>("walk_local_dir", { localRoot: dir });
            if (walked.length === 0) { notice = t("sftp.folder_empty"); return; }
            const folderName = basename(dir);
            for (const w of walked) {
                await transfers.startUpload({
                    sessionId: meta.sessionId,
                    localPath:  joinLocal(dir, w.rel_path),
                    remotePath: joinRemote(cwd, folderName, w.rel_path),
                });
            }
            notice = t("sftp.queued_n", { n: walked.length });
        } catch (err: any) {
            error = errMsg(err);
        }
    }

    /** Mobile single-file upload: pick a source via the dialog plugin and stream
     *  its content:// URI through the shared queue. The remote filename is
     *  recovered from the URI (SAF encodes the display name for user-visible
     *  providers); opaque providers fall back to a timestamped name. */
    async function uploadFile() {
        error = "";
        notice = "";
        if (!meta.sessionId) { error = "Missing SSH session"; return; }
        try {
            const { open } = await import("@tauri-apps/plugin-dialog");
            const src = await open({ multiple: false, directory: false });
            if (!src || Array.isArray(src)) return;
            const name = remoteUploadName(src) || `upload-${fileStamp()}`;
            await transfers.startUpload({
                sessionId: meta.sessionId,
                localPath:  src,
                remotePath: joinRemote(cwd, name),
            });
            notice = t("sftp.queued_n", { n: 1 });
        } catch (err: any) {
            error = errMsg(err);
        }
    }

</script>

<div class="sftp">
    <div class="toolbar">
        <span class="title">SFTP</span>
        <span class="grow"></span>
        <button type="button" class="btn-icon" onclick={() => app.closeSftp()} aria-label={t("common.close")} title={t("common.close")}>×</button>
    </div>
    <div class="header">
        <button class="btn btn-sm" onclick={goUp}>{t("sftp.up")}</button>
        <button class="btn btn-sm" onclick={() => listDir(cwd)}>{t("sftp.refresh")}</button>
        {#if app.isMobile}
            <!-- Mobile: single-file upload only — folder / multi-select upload
                 need the desktop-only folder picker. -->
            <button class="btn btn-sm" disabled={!sftpId} onclick={uploadFile}>
                {t("sftp.upload")}
            </button>
        {:else}
        <div class="upload-wrap" bind:this={uploadWrapEl}>
            <button class="btn btn-sm" disabled={!sftpId} onclick={toggleUploadMenu} aria-haspopup="menu" aria-expanded={uploadMenuOpen}>
                {t("sftp.upload")} <span class="caret">▾</span>
            </button>
            {#if uploadMenuOpen}
                <div class="upload-menu" role="menu">
                    <button role="menuitem" onclick={() => { closeUploadMenu(); uploadFiles(); }}>{t("sftp.upload_files")}</button>
                    <button role="menuitem" onclick={() => { closeUploadMenu(); uploadFolder(); }}>{t("sftp.upload_folder")}</button>
                </div>
            {/if}
        </div>
        {/if}
        {#if !app.isMobile}
        <button class="btn btn-sm" disabled={selectedCount === 0 || !sftpId} onclick={downloadSelected}>
            {selectedCount > 0 ? t("sftp.download_n", { n: selectedCount }) : t("sftp.download")}
        </button>
        {/if}
    </div>
    <input
        type="text"
        class="breadcrumb-input"
        bind:value={pathInput}
        onkeydown={onPathKeyDown}
        disabled={!sftpId}
        spellcheck="false"
        autocomplete="off"
        autocapitalize="off"
        aria-label="Path"
    />

    {#if error}
        <div class="error-banner">{error}</div>
    {/if}
    {#if notice}
        <div class="notice-banner">{notice}</div>
    {/if}

    {#if loading}
        <p class="loading">{t("sftp.loading")}</p>
    {:else}
        <div class="file-list" class:mobile={app.isMobile} oncontextmenu={onSftpContextMenu}>
            <div class="file-row file-header">
                <span class="cell-check">
                    <input
                        type="checkbox"
                        bind:this={selectAllEl}
                        checked={allSelected}
                        disabled={entries.length === 0}
                        onchange={toggleAll}
                        aria-label={t("sftp.select_all")}
                    />
                </span>
                <span class="cell-name h-label">
                    <button type="button" class="sort-btn" onclick={() => toggleSort("name")} title={t("sftp.sort_by_name")}>
                        {t("sftp.column.name")} <span class="sort-arrow">{sortArrow("name")}</span>
                    </button>
                </span>
                <span class="cell-size h-label">
                    <button type="button" class="sort-btn" onclick={() => toggleSort("size")} title={t("sftp.sort_by_size")}>
                        {t("sftp.column.size")} <span class="sort-arrow">{sortArrow("size")}</span>
                    </button>
                </span>
                <span class="cell-mtime h-label">
                    <button type="button" class="sort-btn" onclick={() => toggleSort("mtime")} title={t("sftp.sort_by_mtime")}>
                        {t("sftp.column.modified")} <span class="sort-arrow">{sortArrow("mtime")}</span>
                    </button>
                </span>
            </div>
            {#each sortedEntries as e (e.name)}
                <div
                    class="file-row"
                    class:dir={e.is_dir}
                    class:selected={selected.has(e.name)}
                    class:revealed={revealed === e.name}
                    class:open={!e.is_dir && openedPaths.has(entryPath(e))}
                    class:open-active={!e.is_dir && entryPath(e) === activeOpenPath}
                    data-reveal={e.name}
                    onclick={() => { revealed = null; }}
                    oncontextmenu={(ev) => onContextMenu(ev, e)}
                    ondblclick={(ev) => {
                        if (!app.isMobile && !e.is_dir && !e.is_symlink) {
                            ev.preventDefault();
                            if (isEditable(e)) openEditor(e); else openPreviewTab(e);
                        }
                    }}
                >
                    <span class="cell-check">
                        <input
                            type="checkbox"
                            checked={selected.has(e.name)}
                            onchange={() => toggleSelected(e.name)}
                            aria-label={t("sftp.select_entry", { name: e.name })}
                        />
                    </span>
                    <button class="file-name cell-name" onclick={() => openEntry(e)} title={e.name}>
                        <span class="file-icon">
                            <AppIcon name={e.is_dir ? "folder" : e.is_symlink ? "link" : "file"} size={15} />
                        </span>
                        <span class="file-label">{e.name}</span>
                    </button>
                    <span class="cell-size">{e.is_dir ? "—" : formatSize(e.size)}</span>
                    <span class="cell-mtime">{formatMtime(e.mtime)}</span>
                </div>
            {:else}
                <p class="empty">{t("sftp.empty_dir")}</p>
            {/each}
        </div>
    {/if}
</div>

{#if ctxMenu}
    <div class="ctx-backdrop"
         onclick={closeCtxMenu}
         oncontextmenu={(e) => { e.preventDefault(); closeCtxMenu(); }}
         role="presentation"></div>
    <div class="ctx-menu surface-menu"
         class:ready={ctxReady}
         bind:this={ctxMenuEl}
         style="left: {ctxMenu.x + ctxDx}px; top: {ctxMenu.y + ctxDy}px;">
        {#if !(app.isMobile && ctxMenu.entry.is_dir)}
            <button class="ctx-item" onclick={() => downloadEntry(ctxMenu!.entry)}>{t("sftp.ctx.download")}</button>
        {/if}
        {#if isEditable(ctxMenu!.entry)}
            <button class="ctx-item" onclick={() => openEditor(ctxMenu!.entry)}>{t("sftp.ctx.edit")}</button>
            <button class="ctx-item" onclick={() => openEditorWindow(ctxMenu!.entry)}>{t("sftp.ctx.edit_window")}</button>
        {/if}
        {#if isPreviewable(ctxMenu!.entry)}
            <button class="ctx-item" onclick={() => openPreview(ctxMenu!.entry)}>{t("sftp.ctx.preview")}</button>
            <button class="ctx-item" onclick={() => openInWindow(ctxMenu!.entry)}>{t("sftp.ctx.open_window")}</button>
        {/if}
        <button class="ctx-item" onclick={() => confirmDelete(ctxMenu!.entry)}>{t("sftp.ctx.delete")}</button>
        <button class="ctx-item" onclick={() => startRename(ctxMenu!.entry)}>{t("sftp.ctx.rename")}</button>
        <div class="ctx-sep"></div>
        <button class="ctx-item" onclick={() => copyPath(ctxMenu!.entry)}>{t("sftp.ctx.copy_path")}</button>
        <button class="ctx-item" onclick={() => copyPathToTerminal(ctxMenu!.entry)}>{t("sftp.ctx.copy_path_terminal")}</button>
        <div class="ctx-sep"></div>
        <button class="ctx-item" onclick={() => showProperties(ctxMenu!.entry)}>{t("sftp.ctx.properties")}</button>
    </div>
{/if}

{#if deleteEntry}
    <Modal onClose={() => { deleteEntry = null; }}>
        <p class="modal-text">{t("sftp.delete.confirm", { name: deleteEntry.name })}</p>
        <div class="modal-actions">
            <button class="btn btn-sm" onclick={() => { deleteEntry = null; }}>{t("common.cancel")}</button>
            <button class="btn btn-sm btn-danger" onclick={doDelete}>{t("common.delete")}</button>
        </div>
    </Modal>
{/if}

{#if renameEntry}
    <Modal onClose={() => { renameEntry = null; }}>
        <p class="modal-title">{t("sftp.rename.title")}</p>
        <input
            type="text"
            class="modal-input"
            bind:this={renameInputEl}
            bind:value={renameValue}
            onkeydown={(e) => { if (e.key === "Enter") doRename(); }}
        />
        <div class="modal-actions">
            <button class="btn btn-sm" onclick={() => { renameEntry = null; }}>{t("common.cancel")}</button>
            <button class="btn btn-sm" onclick={doRename}>{t("sftp.rename.title")}</button>
        </div>
    </Modal>
{/if}

{#if propsStat || propsLoading}
    <Modal onClose={closeProperties} style="min-width: 300px;">
        {#if propsLoading}
            <p class="loading">{t("common.loading")}</p>
        {:else if propsStat}
            <p class="props-filename">{propsStat.name}</p>
            <div class="props-spacer"></div>
            {#if !propsStat.is_dir}
                <div class="props-row">
                    <span class="props-label">{t("sftp.props.size")}</span>
                    <span class="props-value">{formatSize(propsStat.size)}</span>
                </div>
            {/if}
            <div class="props-row">
                <span class="props-label">{t("sftp.props.modified")}</span>
                <span class="props-value">{formatMtime(propsStat.mtime)}</span>
            </div>
            <div class="props-row">
                <span class="props-label">{t("sftp.props.owner")}</span>
                <span class="props-value">{propsStat.user ?? (propsStat.uid !== undefined && propsStat.uid !== null ? String(propsStat.uid) : "—")}</span>
            </div>
            <div class="props-row">
                <span class="props-label">{t("sftp.props.group")}</span>
                <span class="props-value">{propsStat.group ?? (propsStat.gid !== undefined && propsStat.gid !== null ? String(propsStat.gid) : "—")}</span>
            </div>
            <div class="props-row">
                <span class="props-label">{t("sftp.props.permissions")}</span>
                <span class="props-value">{formatPermissions(propsStat.permissions)}</span>
            </div>
        {/if}
        <div class="modal-actions" style="margin-top: 16px;">
            <button class="btn btn-sm" onclick={closeProperties}>{t("sftp.props.ok")}</button>
        </div>
    </Modal>
{/if}

{#if previewEntry}
    <Modal onClose={() => (previewEntry = null)} style="max-width: 94vw; width: 82vw; min-width: 60vw; max-height: 90vh;">
        {#key previewEntry.name + previewEntry.size}
            <div style="height: calc(90vh - 140px); min-height: 260px;">
                <PreviewPane
                    name={previewEntry.name}
                    sftpId={sftpId ?? ""}
                    path={entryPath(previewEntry)}
                    size={previewEntry.size}
                    onClose={() => (previewEntry = null)}
                    onOpenWindow={() => openInWindow(previewEntry)}
                />
            </div>
        {/key}
    </Modal>
{/if}

<style>
    .sftp {
        display: flex;
        flex-direction: column;
        height: 100%;
        padding: 12px 14px;
        box-sizing: border-box;
        overflow-y: auto;
        container-type: inline-size;
        /* aside 把 SFTP 收成侧边栏；不再做 max-width 居中。窄宽度下让按钮换行而不是溢出。 */
    }

    .toolbar {
        display: flex;
        align-items: center;
        gap: 8px;
        padding-bottom: 8px;
        border-bottom: 1px solid var(--divider);
        margin-bottom: 10px;
    }

    .title {
        font-size: 13px;
        font-weight: 600;
        color: var(--text-sub);
        letter-spacing: 0.4px;
    }

    .grow { flex: 1; }

    .btn-icon {
        width: 24px;
        height: 24px;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        border: none;
        background: transparent;
        color: var(--text-sub);
        font-size: 18px;
        line-height: 1;
        border-radius: var(--radius-sm);
        cursor: pointer;
    }
    .btn-icon:hover { color: var(--text); background: var(--accent-soft); }

    .header {
        display: flex;
        align-items: center;
        gap: 8px;
        margin-bottom: 8px;
        flex-wrap: wrap;
    }

    .upload-wrap {
        position: relative;
        display: inline-flex;
    }
    .caret {
        font-size: 9px;
        margin-left: 2px;
        opacity: 0.75;
    }
    .upload-menu {
        position: absolute;
        top: calc(100% + 4px);
        left: 0;
        z-index: 10;
        background: var(--surface);
        border: 1px solid var(--divider);
        border-radius: var(--radius-sm);
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.18);
        min-width: 140px;
        padding: 4px;
        display: flex;
        flex-direction: column;
        gap: 2px;
    }
    .upload-menu button {
        background: transparent;
        border: none;
        text-align: left;
        font: inherit;
        color: var(--text);
        padding: 6px 10px;
        border-radius: var(--radius-sm);
        cursor: pointer;
    }
    .upload-menu button:hover {
        background: var(--accent-soft);
    }

    .breadcrumb-input {
        font-family: var(--term-font);
        font-size: 12px;
        color: var(--text);
        padding: calc(6px * var(--density)) calc(10px * var(--density));
        margin-bottom: calc(8px * var(--density));
        background: var(--bg);
        box-shadow: var(--pressed);
        border: none;
        border-radius: var(--radius-sm);
        outline: none;
        width: 100%;
        box-sizing: border-box;
    }
    .breadcrumb-input:focus {
        box-shadow: var(--pressed), 0 0 0 1px var(--accent);
    }

    .error-banner {
        background: color-mix(in srgb, var(--error) 10%, transparent);
        border-left: 3px solid var(--error);
        color: var(--error);
        padding: 8px 12px;
        border-radius: var(--radius-sm);
        margin-bottom: 8px;
        font-size: 12px;
        white-space: pre-line;
    }

    .notice-banner {
        background: color-mix(in srgb, var(--success) 10%, transparent);
        border-left: 3px solid var(--success);
        color: var(--success);
        padding: 8px 12px;
        border-radius: var(--radius-sm);
        margin-bottom: 8px;
        font-size: 12px;
    }

    .loading {
        text-align: center;
        color: var(--text-dim);
        padding: 24px;
    }

    .file-list {
        display: flex;
        flex-direction: column;
        gap: 2px;
    }

    .file-row {
        display: grid;
        grid-template-columns: 24px 1fr 60px 90px;
        align-items: center;
        gap: 8px;
        padding: 6px 8px;
        border-radius: var(--radius-sm);
        transition: background 0.1s;
    }

    .file-row:not(.file-header):hover {
        background: color-mix(in srgb, var(--text-sub) 15%, transparent);
    }
    .file-row.selected {
        background: color-mix(in srgb, var(--accent) 12%, transparent);
    }
    .file-row.selected:hover {
        background: color-mix(in srgb, var(--accent) 18%, transparent);
    }

    /* Row revealed from the editor tab (Ctrl+click on the filename). */
    .file-row.revealed {
        background: color-mix(in srgb, var(--accent) 24%, transparent);
        box-shadow: inset 0 0 0 1px var(--accent);
    }
    .file-row.revealed .file-label {
        color: var(--accent);
        font-weight: 600;
    }

    /* Files open in a tab keep a persistent marker — the user often opens many
       similarly-named figures and needs to spot the one just opened at a
       glance (VS Code explorer style). */
    .file-row.open {
        background: color-mix(in srgb, var(--accent) 10%, transparent);
    }
    .file-row.open .file-label {
        font-weight: 600;
    }
    .file-row.open .file-label::after {
        content: "●";
        margin-left: 6px;
        font-size: 9px;
        vertical-align: 1px;
        color: var(--accent);
    }
    .file-row.open-active {
        background: color-mix(in srgb, var(--accent) 22%, transparent);
        box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 45%, transparent);
    }
    .file-row.open:hover,
    .file-row.open-active:hover {
        background: color-mix(in srgb, var(--accent) 20%, transparent);
    }

    .file-header {
        font-size: 11px;
        color: var(--text-dim);
        letter-spacing: 0.4px;
        text-transform: uppercase;
        border-bottom: 1px solid var(--divider);
        padding-bottom: 6px;
        margin-bottom: 2px;
    }

    .sort-btn {
        display: inline-flex;
        align-items: center;
        gap: 3px;
        padding: 0;
        border: none;
        background: transparent;
        color: inherit;
        font: inherit;
        letter-spacing: inherit;
        text-transform: inherit;
        cursor: pointer;
    }
    .sort-btn:hover {
        color: var(--accent, #8bc8ea);
    }
    .sort-arrow {
        font-size: 9px;
        min-width: 10px;
        color: var(--accent, #8bc8ea);
    }
    .h-label { user-select: none; }
    .cell-size.h-label, .cell-mtime.h-label { text-align: right; }

    .cell-check {
        display: flex;
        align-items: center;
        justify-content: center;
    }
    .cell-check input {
        cursor: pointer;
        margin: 0;
    }

    /* No explicit grid-column: auto-placement keeps name/size in DOM order so
       mobile (checkbox column hidden) lands name in col 1, not the size slot. */
    .file-name {
        border: none;
        background: none;
        text-align: left;
        font-family: inherit;
        font-size: 13px;
        color: var(--text);
        cursor: pointer;
        display: flex;
        align-items: center;
        gap: 6px;
        min-width: 0; /* let .file-label ellipsis-truncate */
        padding: 0;
    }
    .file-label {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .file-row.dir .file-name {
        font-weight: 600;
        color: var(--accent);
    }

    .file-icon {
        display: inline-flex;
        flex-shrink: 0;
    }

    .cell-size {
        font-size: 11px;
        color: var(--text-dim);
        text-align: right;
    }
    .cell-mtime {
        font-size: 11px;
        color: var(--text-dim);
        text-align: right;
        white-space: nowrap;
    }

    /* Narrow widths: drop the mtime column first. Size always stays — for
       multi-file downloads users care most about file size. */
    @container (max-width: 360px) {
        .file-row {
            grid-template-columns: 24px 1fr 60px;
        }
        .cell-mtime { display: none; }
    }

    /* Mobile: drop the checkbox column (multi-select download targets a local
       folder, which is desktop-only) and mtime — keep the row to name + size.
       `display:none` removes the cells from grid placement, so 2 columns suffice. */
    .file-list.mobile .file-row {
        grid-template-columns: 1fr 60px;
    }
    .file-list.mobile .cell-check,
    .file-list.mobile .cell-mtime {
        display: none;
    }

    .empty {
        text-align: center;
        color: var(--text-dim);
        padding: 24px;
    }

    /* ── Context menu (matches TabContextMenu style) ── */

    .ctx-backdrop {
        position: fixed;
        inset: 0;
        z-index: 500;
    }

    .ctx-menu {
        position: fixed;
        z-index: 501;
        min-width: 180px;
        padding: calc(4px * var(--density));
        display: flex;
        flex-direction: column;
        gap: 1px;
        visibility: hidden;
    }
    .ctx-menu.ready {
        visibility: visible;
    }

    .ctx-item {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 20px;
        padding: 7px 12px;
        border: none;
        border-radius: var(--radius-sm);
        background: transparent;
        color: var(--text);
        font-family: inherit;
        font-size: 13px;
        text-align: left;
        cursor: pointer;
        white-space: nowrap;
    }
    .ctx-item:hover:not(:disabled) {
        background: color-mix(in srgb, var(--text) 8%, transparent);
    }

    .ctx-sep {
        height: 1px;
        background: var(--divider);
        margin: 4px 6px;
    }

    /* ── Modal content (scrim + card shell live in Modal.svelte) ── */

    .modal-text {
        font-size: 13px;
        margin: 0 0 16px;
        line-height: 1.5;
    }

    .modal-title {
        font-size: 13px;
        font-weight: 600;
        margin: 0 0 12px;
    }

    .modal-input {
        width: 100%;
        box-sizing: border-box;
        font-family: var(--term-font);
        font-size: 12px;
        padding: 6px 8px;
        border: 1px solid var(--divider);
        border-radius: var(--radius-sm);
        background: var(--bg);
        color: var(--text);
        outline: none;
        margin-bottom: 12px;
    }
    .modal-input:focus {
        border-color: var(--accent);
    }

    .btn-danger {
        color: var(--error) !important;
    }

    /* ── Properties dialog ── */

    .props-filename {
        font-size: 14px;
        font-weight: 600;
        margin: 0 0 4px;
        word-break: break-all;
    }

    .props-spacer {
        height: 12px;
    }

    .props-row {
        display: flex;
        justify-content: space-between;
        align-items: baseline;
        padding: 4px 0;
        font-size: 12px;
    }

    .props-label {
        color: var(--text-sub);
        flex-shrink: 0;
        margin-right: 16px;
    }

    .props-value {
        color: var(--text);
        text-align: right;
        word-break: break-all;
    }

</style>
