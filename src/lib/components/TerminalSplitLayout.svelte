<script lang="ts">
    import {onDestroy} from "svelte";
    import * as app from "../stores/app.svelte.ts";
    import type {SplitDirection, TerminalLayout} from "../terminal/layout.ts";
    import TerminalPane from "./TerminalPane.svelte";

    let {
        layout,
        activePaneId,
        onActivate,
        onResize,
        onClose,
        onContextMenu,
        onInitialConnectionFailure,
    }: {
        layout: TerminalLayout;
        activePaneId: string;
        onActivate: (tabId: string) => void;
        onResize: (path: number[], ratio: number) => void;
        onClose: (tabId: string) => void;
        onContextMenu: (event: MouseEvent, tabId: string) => void;
        onInitialConnectionFailure: (tabId: string, error: unknown) => boolean;
    } = $props();

    type ResizeState = {
        separator: HTMLElement;
        container: HTMLElement;
        pointerId: number;
        path: number[];
        direction: SplitDirection;
    };

    let resizeState: ResizeState | null = null;

    function stopResize(event?: PointerEvent) {
        const state = resizeState;
        if (!state || (event && event.pointerId !== state.pointerId)) return;

        window.removeEventListener("pointermove", handleResizeMove);
        window.removeEventListener("pointerup", stopResize);
        window.removeEventListener("pointercancel", stopResize);
        if (state.separator.hasPointerCapture(state.pointerId)) {
            state.separator.releasePointerCapture(state.pointerId);
        }
        resizeState = null;
    }

    function isRenderableLayout(node: TerminalLayout): boolean {
        if (node.kind === "leaf") {
            const tab = app.tabs().find((candidate) => candidate.id === node.tabId);
            return !!tab && app.isTerminalTabType(tab.type);
        }
        return isRenderableLayout(node.first) || isRenderableLayout(node.second);
    }

    function handleResizeMove(event: PointerEvent) {
        const state = resizeState;
        if (!state || event.pointerId !== state.pointerId) return;

        const rect = state.container.getBoundingClientRect();
        const size = state.direction === "horizontal" ? rect.width : rect.height;
        if (size <= 0 || !Number.isFinite(size)) return;

        const offset = state.direction === "horizontal"
            ? event.clientX - rect.left
            : event.clientY - rect.top;
        const ratio = offset / size;
        if (!Number.isFinite(ratio)) return;

        event.preventDefault();
        onResize([...state.path], ratio);
    }

    function startResize(event: PointerEvent, path: number[], direction: SplitDirection) {
        if (resizeState || event.button !== 0) return;

        const separator = event.currentTarget as HTMLElement;
        const container = separator.parentElement;
        if (!container) return;

        event.preventDefault();
        event.stopPropagation();
        separator.setPointerCapture(event.pointerId);
        resizeState = {
            separator,
            container,
            pointerId: event.pointerId,
            path: [...path],
            direction,
        };
        window.addEventListener("pointermove", handleResizeMove);
        window.addEventListener("pointerup", stopResize);
        window.addEventListener("pointercancel", stopResize);
    }

    onDestroy(stopResize);
</script>

<div class="terminal-split-layout">
    {#snippet renderLayout(node: TerminalLayout, path: number[])}
        {#if node.kind === "leaf"}
            {@const tab = app.tabs().find((candidate) => candidate.id === node.tabId)}
            {#if tab && app.isTerminalTabType(tab.type)}
                {#key tab.id}
                <section
                    class="pane"
                    class:active={activePaneId === tab.id}
                    onclick={() => onActivate(tab.id)}
                    oncontextmenu={(event) => {
                        event.stopPropagation();
                        onContextMenu(event, tab.id);
                    }}
                >
                    <header class="pane-header">
                        <span class="pane-title">{tab.label}</span>
                        <span
                            class="session-status"
                            class:connected={!!app.sessionIdForTab(tab.id)}
                            class:disconnected={!app.sessionIdForTab(tab.id)}
                        >
                            {app.sessionIdForTab(tab.id) ? "Connected" : "Disconnected"}
                        </span>
                        <button
                            class="close-button"
                            type="button"
                            aria-label={`Close ${tab.label}`}
                            title="Close pane"
                            onclick={(event) => {
                                event.stopPropagation();
                                onClose(tab.id);
                            }}
                        >
                            &times;
                        </button>
                    </header>
                    <div class="pane-content">
                        <TerminalPane
                            tabId={tab.id}
                            tabType={tab.type}
                            meta={tab.meta ?? {}}
                            {onInitialConnectionFailure}
                        />
                    </div>
                </section>
                {/key}
            {/if}
        {:else}
            {@const firstRenderable = isRenderableLayout(node.first)}
            {@const secondRenderable = isRenderableLayout(node.second)}
            {#if firstRenderable && secondRenderable}
                <div
                    class="split"
                    class:horizontal={node.direction === "horizontal"}
                    class:vertical={node.direction === "vertical"}
                    style={`--split-ratio: ${node.ratio};`}
                >
                    <div class="split-child first">
                        {@render renderLayout(node.first, [...path, 0])}
                    </div>
                    <div
                        class="separator"
                        class:horizontal={node.direction === "horizontal"}
                        class:vertical={node.direction === "vertical"}
                        role="separator"
                        aria-label="Resize panes"
                        aria-orientation={node.direction === "horizontal" ? "vertical" : "horizontal"}
                        onpointerdown={(event) => startResize(event, path, node.direction)}
                    ></div>
                    <div class="split-child second">
                        {@render renderLayout(node.second, [...path, 1])}
                    </div>
                </div>
            {:else if firstRenderable}
                {@render renderLayout(node.first, [...path, 0])}
            {:else if secondRenderable}
                {@render renderLayout(node.second, [...path, 1])}
            {/if}
        {/if}
    {/snippet}

    {@render renderLayout(layout, [])}
</div>

<style>
    .terminal-split-layout {
        display: flex;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }

    .split {
        display: flex;
        flex: 1 1 auto;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }

    .split.horizontal {
        flex-direction: row;
    }

    .split.vertical {
        flex-direction: column;
    }

    .split-child {
        display: flex;
        flex: calc(1 - var(--split-ratio)) 1 0;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }

    .split-child.first {
        flex: var(--split-ratio) 1 0;
    }

    .split-child > :global(*) {
        flex: 1 1 auto;
        min-width: 0;
        min-height: 0;
    }

    .separator {
        flex: 0 0 6px;
        z-index: 1;
        touch-action: none;
        background: var(--divider);
    }

    .separator.horizontal {
        cursor: col-resize;
    }

    .separator.vertical {
        cursor: row-resize;
    }

    .separator:hover,
    .separator:active {
        background: var(--accent);
    }

    .pane {
        display: flex;
        flex: 1 1 auto;
        flex-direction: column;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
        background: var(--bg);
    }

    .pane-header {
        display: flex;
        align-items: center;
        flex: 0 0 28px;
        gap: 8px;
        min-width: 0;
        padding: 0 8px;
        overflow: hidden;
        border-bottom: 1px solid var(--divider);
        background: var(--surface);
        color: var(--text);
        cursor: pointer;
        user-select: none;
    }

    .pane.active .pane-header {
        border-bottom-color: var(--accent);
    }

    .pane-title {
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-size: 12px;
        font-weight: 600;
    }

    .session-status {
        flex: 0 0 auto;
        color: var(--text-dim);
        font-size: 10px;
        white-space: nowrap;
    }

    .session-status.connected {
        color: var(--success);
    }

    .session-status.disconnected {
        color: var(--text-dim);
    }

    .close-button {
        flex: 0 0 auto;
        width: 22px;
        height: 22px;
        margin-left: auto;
        padding: 0;
        border: 0;
        border-radius: var(--radius-sm);
        background: transparent;
        color: var(--text-dim);
        cursor: pointer;
        font: inherit;
        line-height: 20px;
    }

    .close-button:hover,
    .close-button:focus-visible {
        background: var(--accent-soft);
        color: var(--text);
    }

    .pane-content {
        display: flex;
        flex: 1 1 auto;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }
</style>
