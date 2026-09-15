<script lang="ts">
    // Terminal pinned beside an editor/preview tab (VS Code split view): lets
    // the user run commands while looking at a figure. Renders the same
    // TerminalPane the terminal tab uses; the SSH session lives in the Rust
    // backend, so mounting/unmounting this panel never disconnects.
    import TerminalPane from "./TerminalPane.svelte";
    import * as app from "../stores/app.svelte.ts";
    import { t } from "../i18n/index.svelte.ts";

    // Which terminal tab is pinned. The select below writes back through the
    // store (props are read-only in Svelte 5).
    let selected = $state(app.sideTerminalTab() ?? "");
    let terminalTabs = $derived(
        app.workspaceTabs().filter((tab) => app.isTerminalTabType(tab.type)),
    );

    // Keep the selection valid: if the pinned tab was closed, fall back to the
    // first terminal or clear the panel.
    $effect(() => {
        if (selected && !terminalTabs.some((tab) => tab.id === selected)) {
            const first = terminalTabs[0];
            selected = first ? first.id : "";
            app.setSideTerminal(first ? first.id : null);
        }
    });

    const current = $derived(terminalTabs.find((tab) => tab.id === selected));

    function onChange() {
        app.setSideTerminal(selected || null);
    }

    function close() {
        selected = "";
        app.setSideTerminal(null);
    }
</script>

<div class="side-terminal">
    <div class="side-terminal-head">
        <select class="side-terminal-select" bind:value={selected} onchange={onChange} aria-label={t("sftp.split.select_terminal")}>
            {#each terminalTabs as tab (tab.id)}
                <option value={tab.id}>{tab.label}</option>
            {/each}
        </select>
        <span class="side-terminal-title">{t("sftp.split.terminal")}</span>
        <button type="button" class="side-terminal-close" onclick={close} title={t("common.close")} aria-label={t("common.close")}>×</button>
    </div>
    {#if current}
        <div class="side-terminal-body">
            <TerminalPane tabId={current.id} tabType={current.type} meta={current.meta ?? {}} />
        </div>
    {:else}
        <p class="side-terminal-empty">{t("sftp.split.no_terminal")}</p>
    {/if}
</div>

<style>
    .side-terminal {
        display: flex;
        flex-direction: column;
        height: 100%;
        min-height: 0;
        box-sizing: border-box;
        border-left: 1px solid var(--divider, #333);
        background: var(--bg, #141414);
    }

    .side-terminal-head {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 6px 10px;
        flex: none;
        border-bottom: 1px solid var(--divider, #333);
    }

    .side-terminal-select {
        flex: 1;
        min-width: 0;
        font-size: 12px;
        padding: 3px 6px;
        border-radius: var(--radius-sm, 6px);
        border: 1px solid var(--border, #444);
        background: var(--input-bg, #1c1c1c);
        color: var(--text, #eee);
    }

    .side-terminal-title {
        font-size: 11px;
        color: var(--text-secondary, #9aa0a6);
        flex: none;
    }

    .side-terminal-close {
        flex: none;
        width: 22px;
        height: 22px;
        border: none;
        border-radius: 4px;
        background: transparent;
        color: var(--text-secondary, #9aa0a6);
        font-size: 14px;
        line-height: 1;
        cursor: pointer;
    }
    .side-terminal-close:hover {
        background: color-mix(in srgb, var(--text-sub, #888) 20%, transparent);
        color: var(--text, #eee);
    }

    .side-terminal-body {
        flex: 1;
        min-height: 0;
    }

    .side-terminal-empty {
        margin: 16px;
        font-size: 12px;
        color: var(--text-secondary, #9aa0a6);
    }
</style>
