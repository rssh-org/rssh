<script lang="ts">
    /**
     * Side plugin region: SidePanel chrome (aside + resize handle + keep-alive
     * per-tab panes) around a vertical stack of plugin cards. Cards size to
     * each plugin's reported content height (bridge "size" notification) — no
     * height cap, the column scrolls when the stack outgrows the window.
     */
    import SidePanel from "../components/SidePanel.svelte";
    import PluginFrame from "./PluginFrame.svelte";
    import { entryUrl, type PluginInfo } from "./store.svelte.ts";
    import type { SizeReport } from "./bridge.ts";
    import { FALLBACK_PLUGIN_SIZE, type HorizontalSide } from "./layout.ts";
    import { t } from "../i18n/index.svelte.ts";

    let {
        side,
        width,
        hidden,
        plugins,
        tabs,
        activeTabId,
        onResizeStart,
        onResetWidth,
    }: {
        side: HorizontalSide;
        width: number;
        hidden: boolean;
        plugins: PluginInfo[];
        /** Keep-alive tab containers: tabId → sessionId. */
        tabs: Array<{ tabId: string; sessionId: string }>;
        activeTabId: string;
        onResizeStart: (e: MouseEvent) => void;
        onResetWidth: () => void;
    } = $props();

    // Card collapse is per plugin (global, not per tab): collapsing a chart
    // you don't care about should hold on every tab.
    let collapsed = $state<Record<string, boolean>>({});

    function toggleCollapse(id: string): void {
        collapsed = { ...collapsed, [id]: !collapsed[id] };
    }

    // Reported content height per plugin id. Plugins without size reporting
    // (no sdk autoSize) keep the fallback so their card still shows something.
    let heights = $state<Record<string, number>>({});

    function onPluginSize(id: string, size: SizeReport): void {
        const height = Math.round(size.height ?? 0);
        if (height > 0 && heights[id] !== height) heights = { ...heights, [id]: height };
    }
</script>

<SidePanel
    {side}
    {width}
    {hidden}
    label={t("plugins.side_region")}
    {onResizeStart}
    onResetWidth={onResetWidth}
    panes={tabs.map((tab) => ({
        id: tab.tabId,
        visible: tab.tabId === activeTabId && !hidden,
    }))}
>
    {#snippet pane(p)}
        {@const tab = tabs.find((entry) => entry.tabId === p.id)}
        <div class="plugin-cards">
            {#if tab}
                {#each plugins as plugin (plugin.id)}
                    {@const src = entryUrl(plugin)}
                    <section class="plugin-card">
                        <header class="plugin-card-header">
                            <button
                                class="collapse-btn"
                                aria-expanded={!collapsed[plugin.id]}
                                onclick={() => toggleCollapse(plugin.id)}
                            >
                                <span class="chevron" class:open={!collapsed[plugin.id]}>&#8250;</span>
                                <span class="name">{plugin.name}</span>
                            </button>
                            <span class="version">{plugin.version}</span>
                        </header>
                        {#if !collapsed[plugin.id] && src}
                            <div class="plugin-card-body" style:height="{heights[plugin.id] ?? FALLBACK_PLUGIN_SIZE}px">
                                <PluginFrame
                                    {plugin}
                                    {src}
                                    sessionId={tab.sessionId}
                                    visible={p.visible}
                                    onSize={(size) => onPluginSize(plugin.id, size)}
                                />
                            </div>
                        {/if}
                    </section>
                {/each}
            {/if}
        </div>
    {/snippet}
</SidePanel>

<style>
    .plugin-cards {
        flex: 1;
        min-height: 0;
        display: flex;
        flex-direction: column;
        overflow-y: auto;
        gap: 1px;
    }
    .plugin-card {
        display: flex;
        flex-direction: column;
        flex: 0 0 auto;
        border-bottom: 1px solid var(--divider);
    }
    .plugin-card:last-child {
        border-bottom: none;
    }
    .plugin-card-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        padding: 4px 10px;
        flex: 0 0 auto;
    }
    .collapse-btn {
        display: flex;
        align-items: center;
        gap: 4px;
        border: none;
        background: none;
        color: var(--text-sub);
        font-family: inherit;
        font-size: 12px;
        padding: 0;
        cursor: pointer;
        min-width: 0;
    }
    .collapse-btn:hover { color: var(--text); }
    .collapse-btn .name {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .chevron {
        display: inline-block;
        transition: transform 0.12s ease;
        color: var(--text-dim);
    }
    .chevron.open { transform: rotate(90deg); }
    .version {
        font-size: 11px;
        color: var(--text-dim);
        flex: 0 0 auto;
    }
    .plugin-card-body {
        /* Height comes from the plugin's reported content size (inline style);
           FALLBACK_PLUGIN_SIZE covers pre-report and never-reporting plugins. */
        flex: 0 0 auto;
        min-height: 0;
    }
</style>
