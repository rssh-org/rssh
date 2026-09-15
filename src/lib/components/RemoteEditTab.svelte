<script lang="ts">
    // Main-window remote file editor tab (VS Code style). Rendered inside the
    // center region of AppShell; the SFTP sidebar stays mounted as the file
    // tree. Reads/writes the shared SFTP session via sftp_download/sftp_upload;
    // closing the tab never disconnects. CSV/TSV get the editable grid, other
    // text/code files get the CodeMirror editor. Reports dirty state so
    // AppShell can intercept tab close (×) with a save/discard/cancel confirm,
    // and forwards Ctrl+click on the filename to reveal the file in the SFTP
    // tree.
    import { onDestroy } from "svelte";
    import EditorPane from "./EditorPane.svelte";
    import TableEditor from "./TableEditor.svelte";
    import * as app from "../stores/app.svelte.ts";
    import { previewKind } from "../sftp-preview.ts";

    let { tabId, meta, active = true }: { tabId: string; meta: Record<string, string>; active?: boolean } = $props();

    const sftpId = meta.sftpId ?? "";
    const path = meta.path ?? "";
    const name = meta.name ?? "edit";
    const sizeText = meta.size;
    const size = sizeText ? Number(sizeText) || undefined : undefined;

    let textMode = $state(previewKind(name) !== "table");
    // Bumped when AppShell asks us to save (tab-close "save and leave"); the
    // editor saves once per bump and only closes on success.
    let saveToken = $state(0);

    function close() {
        app.closeTab(tabId);
    }

    function reveal() {
        app.requestRevealFile(sftpId, path);
    }

    $effect(() => {
        const req = app.saveRequest();
        if (!req || req.id !== tabId) return;
        app.clearSaveRequest(req.nonce);
        saveToken++;
    });

    // Editor reports success through the dirty channel: after a successful
    // save dirty goes false; a failed save keeps dirty true so the tab stays.
    $effect(() => {
        if (saveToken > 0 && !app.isTabDirty(tabId)) {
            app.closeTab(tabId);
        }
    });

    onDestroy(() => {
        app.setTabDirty(tabId, false);
    });
</script>

<div class="remote-edit-tab">
    {#key path + "|" + textMode}
        {#if previewKind(name) === "table" && !textMode}
            <TableEditor
                {name}
                {sftpId}
                {path}
                {size}
                {active}
                {saveToken}
                onClose={close}
                onSwitchText={() => (textMode = true)}
                onDirtyChange={(d) => app.setTabDirty(tabId, d)}
            />
        {:else}
            <EditorPane
                {name}
                {sftpId}
                {path}
                {size}
                {active}
                {saveToken}
                onClose={close}
                onReveal={reveal}
                onDirtyChange={(d) => app.setTabDirty(tabId, d)}
            />
        {/if}
    {/key}
</div>

<style>
    .remote-edit-tab {
        height: 100%;
        min-height: 0;
        box-sizing: border-box;
        padding: 12px 16px 16px;
        overflow: hidden;
    }
</style>
