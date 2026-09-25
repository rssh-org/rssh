<script lang="ts">
    // Main-window remote preview tab (VS Code style): image / PDF / markdown /
    // table / code previews open as tabs in the tab bar, so previously opened
    // files stay visible and switchable. The SFTP sidebar stays mounted as the
    // file tree; closing the tab never disconnects the session.
    import PreviewPane from "./PreviewPane.svelte";
    import * as app from "../stores/app.svelte.ts";

    let { tabId, meta }: { tabId: string; meta: Record<string, string> } = $props();

    const sftpId = meta.sftpId ?? "";
    const path = meta.path ?? "";
    const name = meta.name ?? "preview";
    const sizeText = meta.size;
    const size = sizeText ? Number(sizeText) || undefined : undefined;

    function close() {
        app.closeTab(tabId);
    }
</script>

<div class="remote-preview-tab">
    <PreviewPane
        {name}
        {sftpId}
        {path}
        {size}
        onClose={close}
    />
</div>

<style>
    .remote-preview-tab {
        height: 100%;
        min-height: 0;
        box-sizing: border-box;
        padding: 12px 16px 16px;
        overflow: hidden;
    }
</style>
