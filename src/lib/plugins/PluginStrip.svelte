<script lang="ts">
    /**
     * Strip plugin region: a single-line bar docked to the top or bottom edge
     * of the terminal area, JetBrains-bottom-bar style. Plugins sit side by
     * side horizontally (not stacked) — a text monitor is one segment of the
     * line. Segments size to each plugin's reported content width (bridge
     * "size" notification); when they don't all fit, the line scrolls
     * horizontally instead of squeezing them. Fixed slim height, no resize
     * handle: the bar is chrome, not content. Same per-tab keep-alive contract
     * as PluginSide.
     */
    import PluginFrame from "./PluginFrame.svelte";
    import { entryUrl, type PluginInfo } from "./store.svelte.ts";
    import type { SizeReport } from "./bridge.ts";
    import { FALLBACK_PLUGIN_SIZE } from "./layout.ts";
    import { t } from "../i18n/index.svelte.ts";

    let {
        position,
        hidden,
        plugins,
        tabs,
        activeTabId,
    }: {
        position: "top" | "bottom";
        /** Collapsed (active tab has no plugin panel open); DOM stays for keep-alive. */
        hidden: boolean;
        plugins: PluginInfo[];
        tabs: Array<{ tabId: string; sessionId: string }>;
        activeTabId: string;
    } = $props();

    // Reported content width per plugin id; FALLBACK_PLUGIN_SIZE covers
    // pre-report and never-reporting plugins so a segment is never zero-wide.
    let widths = $state<Record<string, number>>({});

    function onPluginSize(id: string, size: SizeReport): void {
        const width = Math.round(size.width ?? 0);
        if (width > 0 && widths[id] !== width) widths = { ...widths, [id]: width };
    }
</script>

<div
    class="plugin-strip"
    class:on-top={position === "top"}
    class:hidden
    aria-label={t("plugins.strip_region")}
>
    {#each tabs as tab (tab.tabId)}
        <div class="plugin-tab-strip" class:visible={tab.tabId === activeTabId}>
            {#each plugins as plugin (plugin.id)}
                {@const src = entryUrl(plugin)}
                {#if src}
                    <section
                        class="plugin-strip-card"
                        style="flex: 0 0 {widths[plugin.id] ?? FALLBACK_PLUGIN_SIZE}px"
                        title={plugin.name}
                    >
                        <PluginFrame
                            {plugin}
                            {src}
                            sessionId={tab.sessionId}
                            visible={tab.tabId === activeTabId && !hidden}
                            onSize={(size) => onPluginSize(plugin.id, size)}
                        />
                    </section>
                {/if}
            {/each}
        </div>
    {/each}
</div>

<style>
    .plugin-strip {
        position: relative;
        flex: 0 0 auto;
        height: 28px;
        background: var(--bg);
        border-top: 1px solid var(--divider);
        overflow: hidden;
    }
    .plugin-strip.on-top {
        border-top: none;
        border-bottom: 1px solid var(--divider);
    }
    .plugin-strip.hidden {
        height: 0 !important;
        overflow: hidden;
        border: none;
    }

    .plugin-tab-strip {
        display: none;
    }
    .plugin-tab-strip.visible {
        display: flex;
        flex-direction: row;
        height: 100%;
        overflow-x: auto;
        overflow-y: hidden;
        /* Scrolls (wheel/trackpad) but shows no scrollbar — a visible bar
           eats into the 28px line. */
        scrollbar-width: none;
    }
    .plugin-tab-strip.visible::-webkit-scrollbar {
        display: none;
    }
    /* Each plugin is a horizontal segment sized to its content (inline style:
       reported width, 180px fallback); segments never squeeze or wrap — the
       line scrolls instead. */
    .plugin-strip-card {
        height: 100%;
        border-left: 1px solid var(--divider);
        min-height: 0;
    }
    .plugin-strip-card:first-child {
        border-left: none;
    }
</style>
