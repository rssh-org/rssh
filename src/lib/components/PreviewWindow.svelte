<script lang="ts">
    // Standalone preview window page. Loaded via
    //   index.html?view=preview&sftp=<id>&path=<remote path>&name=<file name>&size=<bytes>
    // The SFTP session lives in the main window; this window only reads it via
    // `sftp_download`, so closing it never tears down the user's connection.
    import { onMount } from "svelte";
    import { getCurrentWindow } from "@tauri-apps/api/window";
    import PreviewPane from "./PreviewPane.svelte";

    const params = new URLSearchParams(window.location.search);
    const name = params.get("name") ?? "preview";
    const sftpId = params.get("sftp") ?? "";
    const path = params.get("path") ?? "";
    const sizeText = params.get("size");
    const size = sizeText ? Number(sizeText) || undefined : undefined;

    const win = getCurrentWindow();
    document.title = name;

    function close() {
        void win.close();
    }

    onMount(() => {
        function onKey(e: KeyboardEvent) {
            if (e.key === "Escape") close();
        }
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    });
</script>

<svelte:head><title>{name}</title></svelte:head>

<div class="preview-window">
    <PreviewPane {name} {sftpId} {path} {size} onClose={close} />
</div>

<style>
    .preview-window {
        height: 100vh;
        box-sizing: border-box;
        padding: 12px 16px 16px;
        background: var(--bg, #141517);
        overflow: hidden;
    }
</style>
