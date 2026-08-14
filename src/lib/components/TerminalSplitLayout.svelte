<script lang="ts">
    import {onDestroy} from "svelte";
    import * as app from "../stores/app.svelte.ts";
    import {
        layoutLeaves,
        normalizeRatio,
        type LayoutLeaf,
        type SplitDirection,
        type TerminalLayout,
    } from "../terminal/layout.ts";
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

    type Bounds = Pick<LayoutLeaf, "left" | "top" | "width" | "height">;
    type RenderedPane = { pane: LayoutLeaf; tab: app.Tab };

    let paneLeaves = $derived.by((): RenderedPane[] => {
        const tabs = app.tabs();
        return layoutLeaves(layout).flatMap((pane) => {
            const tab = tabs.find((candidate) => candidate.id === pane.tabId);
            return tab && app.isTerminalTabType(tab.type) ? [{ pane, tab }] : [];
        });
    });

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

    function boundsStyle(bounds: Bounds): string {
        return `left:${bounds.left * 100}%;top:${bounds.top * 100}%;width:${bounds.width * 100}%;height:${bounds.height * 100}%;`;
    }

    onDestroy(stopResize);
</script>

<div class="terminal-split-layout">
    {#each paneLeaves as { pane, tab } (tab.id)}
        <div
            class="pane"
            class:active={activePaneId === tab.id}
            style={boundsStyle(pane)}
            role="button"
            tabindex="0"
            aria-label={`Activate ${tab.label}`}
            onclick={() => onActivate(tab.id)}
            onkeydown={(event) => {
                if (event.currentTarget !== event.target) return;
                if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault();
                    onActivate(tab.id);
                }
            }}
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
        </div>
    {/each}

    {#snippet renderSplits(node: TerminalLayout, bounds: Bounds, path: number[])}
        {#if node.kind === "split"}
            {@const ratio = normalizeRatio(node.ratio)}
            {@const firstBounds = node.direction === "horizontal"
                ? { left: bounds.left, top: bounds.top, width: bounds.width * ratio, height: bounds.height }
                : { left: bounds.left, top: bounds.top, width: bounds.width, height: bounds.height * ratio }}
            {@const secondBounds = node.direction === "horizontal"
                ? { left: bounds.left + bounds.width * ratio, top: bounds.top, width: bounds.width * (1 - ratio), height: bounds.height }
                : { left: bounds.left, top: bounds.top + bounds.height * ratio, width: bounds.width, height: bounds.height * (1 - ratio) }}
            <div class="split-region" style={boundsStyle(bounds)}>
                <div
                    class="separator"
                    class:horizontal={node.direction === "horizontal"}
                    class:vertical={node.direction === "vertical"}
                    style={node.direction === "horizontal"
                        ? `left:calc(${ratio * 100}% - 3px);top:0;width:6px;height:100%;`
                        : `left:0;top:calc(${ratio * 100}% - 3px);width:100%;height:6px;`}
                    role="separator"
                    aria-label="Resize panes"
                    aria-orientation={node.direction === "horizontal" ? "vertical" : "horizontal"}
                    onpointerdown={(event) => startResize(event, path, node.direction)}
                ></div>
            </div>
            {@render renderSplits(node.first, firstBounds, [...path, 0])}
            {@render renderSplits(node.second, secondBounds, [...path, 1])}
        {/if}
    {/snippet}

    {@render renderSplits(layout, { left: 0, top: 0, width: 1, height: 1 }, [])}
</div>

<style>
    .terminal-split-layout {
        position: relative;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }

    .split-region {
        position: absolute;
        pointer-events: none;
        z-index: 2;
    }

    .separator {
        position: absolute;
        pointer-events: auto;
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
        position: absolute;
        display: flex;
        flex-direction: column;
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
