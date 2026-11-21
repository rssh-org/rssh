<script lang="ts">
    import { exportDownload, listDownloads, type SavedDownload } from "../file-access.ts";
    import { errCoded, errMsg, t } from "../i18n/index.svelte.ts";
    import { downloadRevision } from "../stores/transfers.svelte.ts";
    import { toast } from "../stores/toast.svelte.ts";

    let { formatSize }: { formatSize: (bytes: number) => string } = $props();
    let files = $state<SavedDownload[]>([]);
    let loading = $state(true);
    let error = $state("");
    let refresh = $state(0);

    $effect(() => {
        downloadRevision();
        refresh;
        let current = true;
        loading = true;
        error = "";
        listDownloads().then(result => {
            if (current) files = result;
        }).catch(cause => {
            if (current) error = errMsg(cause);
        }).finally(() => {
            if (current) loading = false;
        });
        return () => { current = false; };
    });

    async function exportFile(file: SavedDownload) {
        try {
            await exportDownload(file);
        } catch (cause) {
            toast.error(errCoded(cause)?.code === "sftp_target_changed"
                ? t("downloads.export_changed", { name: file.name }) : errMsg(cause));
            // A same-name download may have replaced the selected version.
            // Refresh the listing so the user can choose the current file.
            refresh++;
        }
    }
</script>

<section aria-label={t("downloads.saved")}>
    <header>
        <h3>{t("downloads.saved")}</h3>
        <button class="btn btn-sm" disabled={loading} onclick={() => refresh++}>{t("sftp.refresh")}</button>
    </header>
    {#if error}<p class="error">{error}</p>{/if}
    {#if files.length}
        <ul>
            {#each files as file (file.localPath)}
                <li>
                    <div>
                        <span class="filename">{file.name}</span>
                        <span class="size">{formatSize(file.size)}</span>
                    </div>
                    <button class="btn btn-sm" onclick={() => exportFile(file)}>
                        {file.isBackup ? t("downloads.export_backup") : t("downloads.export")}
                    </button>
                </li>
            {/each}
        </ul>
    {:else if loading}
        <p>{t("common.loading")}</p>
    {:else if !error}
        <p>{t("downloads.saved_empty")}</p>
    {/if}
</section>

<style>
    section { padding-top: 8px; }
    header, li { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
    h3 { margin: 0; font-size: 13px; color: var(--text); }
    ul { list-style: none; }
    li { padding: 8px 0; border-bottom: 1px solid var(--divider); }
    li:last-child { border-bottom: 0; }
    li > div { min-width: 0; }
    button { flex-shrink: 0; }
    .filename { display: block; overflow-wrap: anywhere; color: var(--text); }
    .size, p { font-size: 12px; color: var(--text-sub); }
    p { margin: 8px 0; }
    .error { color: var(--error); overflow-wrap: anywhere; }
</style>
