<script lang="ts">
    // VS Code style remote file tree for the standalone edit window. Lazily lists
    // directories via `sftp_list`; clicking a file asks the parent to open it.
    import { invoke } from "@tauri-apps/api/core";
    import { errMsg, t } from "../i18n/index.svelte.ts";
    import AppIcon from "./AppIcon.svelte";

    interface TreeEntry {
        name: string;
        is_dir: boolean;
        is_symlink: boolean;
        size: number;
        mtime: number;
    }

    let {
        sftpId,
        startDir,
        activePath,
        onOpen,
    }: {
        sftpId: string;
        startDir: string;
        activePath: string;
        onOpen: (path: string, name: string, size: number) => void;
    } = $props();

    let expanded = $state<Set<string>>(new Set([startDir]));
    let children = $state<Record<string, TreeEntry[]>>({});
    let loadingDirs = $state<Set<string>>(new Set());
    let error = $state("");

    async function toggle(dir: string) {
        if (expanded.has(dir)) {
            expanded = new Set([...expanded].filter((d) => d !== dir));
            return;
        }
        expanded = new Set([...expanded, dir]);
        if (children[dir] === undefined && !loadingDirs.has(dir)) {
            await load(dir);
        }
    }

    async function load(dir: string) {
        loadingDirs = new Set([...loadingDirs, dir]);
        error = "";
        try {
            const list = await invoke<TreeEntry[]>("sftp_list", { sftpId, path: dir });
            const sorted = list
                .filter((e) => !e.is_symlink || e.is_dir) // symlink-to-dir ok, symlink-to-file skip
                .sort((a, b) => {
                    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
                    return a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
                });
            children = { ...children, [dir]: sorted };
        } catch (e: any) {
            error = errMsg(e);
        } finally {
            loadingDirs = new Set([...loadingDirs].filter((d) => d !== dir));
        }
    }

    function join(dir: string, name: string): string {
        return dir.endsWith("/") ? dir + name : dir + "/" + name;
    }

    /** Recursive renderer: a directory row plus its (expanded) children. */
    function renderDir(dir: string, depth: number): { path: string; name: string; depth: number; isDir: boolean }[] {
        const out: { path: string; name: string; depth: number; isDir: boolean }[] = [];
        out.push({ path: dir, name: dir.split("/").filter(Boolean).pop() ?? "/", depth, isDir: true });
        if (!expanded.has(dir)) return out;
        const list = children[dir] ?? [];
        for (const e of list) {
            const p = join(dir, e.name);
            if (e.is_dir) out.push(...renderDir(p, depth + 1));
            else out.push({ path: p, name: e.name, depth: depth + 1, isDir: false });
        }
        return out;
    }

    const tree = $derived(renderDir(startDir, 0));
</script>

<div class="file-tree">
    <div class="tree-head">
        <span class="tree-title" title={startDir}>{startDir.split("/").filter(Boolean).pop() ?? "/"}</span>
        <button type="button" class="btn-icon tree-reload" onclick={() => void load(startDir)} title={t("sftp.refresh")} aria-label={t("sftp.refresh")}>⟳</button>
    </div>
    {#if error}
        <p class="tree-error">{error}</p>
    {/if}
    <div class="tree-body">
        {#each tree as node}
            {#if node.isDir}
                <div
                    class="tree-row"
                    class:active={activePath === node.path}
                    style="padding-left: {8 + node.depth * 14}px;"
                    onclick={() => void toggle(node.path)}
                    title={node.path}
                >
                    <span class="tree-arrow">{expanded.has(node.path) ? "▾" : "▸"}</span>
                    <span class="tree-icon"><AppIcon name="folder" size={14} /></span>
                    <span class="tree-label">{node.name}</span>
                </div>
            {:else}
                <div
                    class="tree-row file"
                    class:active={activePath === node.path}
                    style="padding-left: {8 + node.depth * 14}px;"
                    onclick={() => onOpen(node.path, node.name, node.size)}
                    title={node.path}
                >
                    <span class="tree-arrow"></span>
                    <span class="tree-icon"><AppIcon name="file" size={14} /></span>
                    <span class="tree-label">{node.name}</span>
                </div>
            {/if}
        {/each}
        {#if loadingDirs.has(startDir)}
            <p class="tree-loading">{t("common.loading")}</p>
        {/if}
    </div>
</div>

<style>
    .file-tree {
        display: flex;
        flex-direction: column;
        height: 100%;
        min-height: 0;
    }

    .tree-head {
        display: flex;
        align-items: center;
        gap: 4px;
        padding: 6px 8px;
        border-bottom: 1px solid var(--border, #2a2d33);
        flex: none;
    }

    .tree-title {
        font-size: 11px;
        font-weight: 600;
        color: var(--text-secondary, #6b7280);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        flex: 1;
        min-width: 0;
    }

    .tree-reload { font-size: 12px; }

    .tree-error {
        margin: 6px 8px;
        font-size: 11px;
        color: var(--danger, #d64545);
    }

    .tree-body {
        flex: 1;
        min-height: 0;
        overflow: auto;
        padding: 4px 0 12px;
    }

    .tree-row {
        display: flex;
        align-items: center;
        gap: 4px;
        padding-top: 2px;
        padding-bottom: 2px;
        padding-right: 8px;
        font-size: 12px;
        color: var(--text);
        cursor: pointer;
        white-space: nowrap;
        user-select: none;
    }

    .tree-row:hover { background: var(--bg-4, #2c3036); }

    .tree-row.active { background: var(--accent-soft, rgba(139, 200, 234, 0.15)); color: var(--accent, #8bc8ea); }

    .tree-arrow { width: 12px; flex: none; font-size: 10px; color: var(--text-secondary, #6b7280); }

    .tree-icon { flex: none; font-size: 11px; opacity: 0.85; }

    .tree-label {
        overflow: hidden;
        text-overflow: ellipsis;
        min-width: 0;
    }

    .tree-loading {
        margin: 6px 10px;
        font-size: 11px;
        color: var(--text-secondary, #6b7280);
    }
</style>
