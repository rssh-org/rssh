<script lang="ts">
    // Standalone edit window. Loaded via
    //   index.html?view=edit&sftp=<id>&path=<remote path>&name=<file name>&size=<bytes>
    // The SFTP session lives in the main window; this window reads/writes it via
    // the shared `sftp_download` / `sftp_upload` commands, so closing it never
    // tears down the user's connection. A VS Code style file tree on the left
    // switches files; CSV/TSV get the editable grid, other text/code files get
    // the CodeMirror editor.
    import { getCurrentWindow } from "@tauri-apps/api/window";
    import EditorPane from "./EditorPane.svelte";
    import TableEditor from "./TableEditor.svelte";
    import FileTree from "./FileTree.svelte";
    import { previewKind } from "../sftp-preview.ts";
    import { t } from "../i18n/index.svelte.ts";

    const params = new URLSearchParams(window.location.search);
    const sftpId = params.get("sftp") ?? "";
    const startPath = params.get("path") ?? "";
    const startName = params.get("name") ?? "edit";
    const sizeText = params.get("size");
    const startSize = sizeText ? Number(sizeText) || undefined : undefined;

    const win = getCurrentWindow();
    document.title = startName;

    // Currently open file (switched from the tree or the initial query).
    let current = $state({ name: startName, path: startPath, size: startSize });
    // CSV/TSV default to the grid; "以文本方式编辑" flips to plain text.
    let textMode = $state(previewKind(startName) !== "table");
    let treeOpen = $state(true);

    function close() {
        void win.close();
    }

    function openFile(path: string, name: string, size: number) {
        current = { name, path, size };
        textMode = previewKind(name) !== "table";
        document.title = name;
    }

    function startDirOf(path: string): string {
        const i = path.lastIndexOf("/");
        return i > 0 ? path.slice(0, i) : "/";
    }
</script>

<svelte:head><title>{current.name}</title></svelte:head>

<div class="edit-window">
    {#if treeOpen}
        <aside class="edit-sidebar">
            <FileTree
                {sftpId}
                startDir={startDirOf(current.path)}
                activePath={current.path}
                onOpen={openFile}
            />
        </aside>
    {/if}
    <div class="edit-main">
        <div class="edit-chrome">
            <button type="button" class="btn-icon" onclick={() => (treeOpen = !treeOpen)}
                title={treeOpen ? t("sftp.tree.hide") : t("sftp.tree.show")}
                aria-label={treeOpen ? t("sftp.tree.hide") : t("sftp.tree.show")}>
                {treeOpen ? "◀" : "▶"}
            </button>
        </div>
        {#key current.path + "|" + textMode}
            {#if previewKind(current.name) === "table" && !textMode}
                <TableEditor
                    name={current.name}
                    {sftpId}
                    path={current.path}
                    size={current.size}
                    onClose={close}
                    onSwitchText={() => (textMode = true)}
                />
            {:else}
                <EditorPane
                    name={current.name}
                    {sftpId}
                    path={current.path}
                    size={current.size}
                    onClose={close}
                />
            {/if}
        {/key}
    </div>
</div>

<style>
    .edit-window {
        display: flex;
        height: 100vh;
        box-sizing: border-box;
        padding: 12px 16px 16px 0;
        background: var(--bg, #141517);
        overflow: hidden;
        gap: 12px;
    }

    .edit-sidebar {
        width: 220px;
        min-width: 160px;
        flex: none;
        border: 1px solid var(--border, #2a2d33);
        border-radius: var(--radius-sm, 6px);
        margin-left: 12px;
        overflow: hidden;
        background: var(--bg-2, #181a1d);
    }

    .edit-main {
        flex: 1;
        min-width: 0;
        display: flex;
        flex-direction: column;
    }

    .edit-chrome {
        flex: none;
        padding: 2px 0 6px;
        display: flex;
        align-items: center;
    }

    .edit-chrome .btn-icon {
        font-size: 11px;
        width: 22px;
        height: 22px;
    }
</style>
