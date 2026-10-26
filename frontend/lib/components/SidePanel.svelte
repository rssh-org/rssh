<script lang="ts">
    /**
     * Generic side-panel chrome shared by AI / SFTP / plugins: an aside docked
     * to a left/right edge of the content row, a col-resize handle on its
     * inner edge (dblclick resets the width preference), and keep-alive
     * per-tab panes — inactive panes are display:none, so mounted instances
     * (chat drafts, SFTP cwd, plugin charts) survive tab switches.
     *
     * Panels differ only in policy, passed as props: what renders inside a
     * pane (snippet), whether the aside carries the divider border (AI's
     * ChatPanel draws its own), and the AI-only mobile takeover.
     */
    import type { Snippet } from "svelte";
    import { t } from "../i18n/index.svelte.ts";

    let {
        side,
        width,
        hidden,
        label,
        bordered = true,
        mobileFullWidth = false,
        onResizeStart,
        onResetWidth,
        panes,
        pane,
    }: {
        /** Dock edge of the content row — decides where the divider/handle sit. */
        side: "left" | "right";
        /** Fitted width in px from the panel-width negotiation. */
        width: number;
        /** Collapse to zero without unmounting panes (keep-alive). */
        hidden: boolean;
        label?: string;
        /** Draw the divider on the inner edge; false when the pane content
         *  carries its own (AI's ChatPanel). */
        bordered?: boolean;
        /** Portrait phones: panel takes over the content area, no resizing. */
        mobileFullWidth?: boolean;
        onResizeStart: (e: MouseEvent) => void;
        onResetWidth: () => void;
        /** One entry per tab that ever opened the panel. */
        panes: Array<{ id: string; visible: boolean }>;
        pane: Snippet<[{ id: string; visible: boolean }]>;
    } = $props();
</script>

<aside
    class="side-panel"
    class:bordered
    class:dock-right={side === "right"}
    class:hidden
    class:mobile-full={mobileFullWidth}
    style="flex: 0 0 {width}px; max-width: {width}px;"
    aria-label={label}
>
    <div
        class="resize-handle"
        onmousedown={onResizeStart}
        ondblclick={onResetWidth}
        role="separator"
        aria-orientation="vertical"
        title={t("common.resize_hint")}
    ></div>
    {#each panes as p (p.id)}
        <div class="pane" class:visible={p.visible}>
            {@render pane(p)}
        </div>
    {/each}
</aside>

<style>
    .side-panel {
        position: relative;
        background: var(--bg);
    }
    .side-panel.bordered {
        border-right: 1px solid var(--divider);
    }
    /* Docked right → inner edge (divider + handle) flips to the left. */
    .side-panel.bordered.dock-right {
        border-right: none;
        border-left: 1px solid var(--divider);
    }
    .side-panel.hidden {
        flex: 0 0 0 !important;
        max-width: 0 !important;
        overflow: hidden;
        border: none;
    }

    /* Resize handle: 6px hit area on the inner edge, hairline on hover/drag. */
    .resize-handle {
        position: absolute;
        top: 0;
        bottom: 0;
        right: -3px;
        width: 6px;
        cursor: col-resize;
        z-index: 10;
        background: transparent;
        transition: background 0.12s ease;
    }
    .dock-right .resize-handle {
        right: auto;
        left: -3px;
    }
    .resize-handle:hover,
    .resize-handle:active {
        background: var(--accent);
        opacity: 0.45;
    }

    /* Keep-alive per tab: only the active tab's pane participates in layout. */
    .pane {
        position: absolute;
        inset: 0;
        display: none;
    }
    .pane.visible {
        display: flex;
        flex-direction: column;
    }

    /* Portrait phones (AI takeover): the panel fills the content area and
       the handle disappears. */
    @media (max-width: 480px) {
        .side-panel.mobile-full:not(.hidden) {
            flex: 1 1 auto !important;
            max-width: none !important;
        }
        .side-panel.mobile-full .resize-handle {
            display: none;
        }
    }
</style>
