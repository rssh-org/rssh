<script lang="ts">
    import * as app from "../stores/app.svelte.ts";
    import * as ai from "../ai/store.svelte.ts";
    import { toast } from "../stores/toast.svelte.ts";
    import { t, errMsg } from "../i18n/index.svelte.ts";
    import AppIcon from "./AppIcon.svelte";
    import { onDestroy } from "svelte";

    function prevent(e: Event) { e.preventDefault(); }

    // Long-press a modifier button to LOCK it (stays armed across keys); a short
    // tap stays one-shot (arms for the next key, then clears). Press state is
    // per-modifier: two fingers can hold Ctrl and Alt at once, and one button's
    // release must never touch the other's pending timer.
    type ModName = "ctrl" | "alt";
    const modPress: Record<ModName, { timer: ReturnType<typeof setTimeout> | null; fired: boolean }> = {
        ctrl: { timer: null, fired: false },
        alt: { timer: null, fired: false },
    };
    const MOD_LONG_PRESS_MS = 400;
    function armLongPress(mod: ModName, lock: () => void) {
        modPress[mod].fired = false;
        modPress[mod].timer = setTimeout(() => { modPress[mod].fired = true; lock(); }, MOD_LONG_PRESS_MS);
    }
    function clearModTimer(mod: ModName) {
        const p = modPress[mod];
        if (p.timer) { clearTimeout(p.timer); p.timer = null; }
    }
    function finishPress(mod: ModName, tap: () => void) {
        clearModTimer(mod);
        if (!modPress[mod].fired) tap();
        modPress[mod].fired = false;
    }
    // pointercancel (system interrupt) fully resets. pointerleave must NOT reset
    // the fired flag: if the long-press already fired, sliding off and back up
    // should still release without toggling the lock off.
    function cancelPress(mod: ModName) {
        clearModTimer(mod);
        modPress[mod].fired = false;
    }
    onDestroy(() => {
        // A tab switch can unmount the bar mid-press; a pending timer must not
        // lock a modifier after the UI is gone.
        clearModTimer("ctrl");
        clearModTimer("alt");
    });

    function send(seq: string) {
        app.sendToTerminal(seq);
        app.clearModifiers();
    }

    function arrow(dir: app.ArrowDir) {
        const ctrl = app.ctrlActive();
        const alt = app.altActive();
        const mod = (ctrl && alt) ? 7 : ctrl ? 5 : alt ? 3 : 0;
        app.sendArrow(dir, mod);
        app.clearModifiers();
    }

    // Extension keys (PgUp/PgDn/Home/End/Ins/Del) plus the panel actions
    // (Snippets/SFTP/AI) tucked behind a "..." button so the main bar stays one
    // row of high-frequency keys; opened as a floating panel above the bar.
    const EXT_KEYS: { label: string; seq: string }[] = [
        { label: "PgUp", seq: "\x1b[5~" },
        { label: "PgDn", seq: "\x1b[6~" },
        { label: "Home", seq: "\x1b[H" },
        { label: "End", seq: "\x1b[F" },
        { label: "Ins", seq: "\x1b[2~" },
        { label: "Del", seq: "\x1b[3~" },
    ];
    let extOpen = $state(false);

    // 当前 tab 是否有活跃 SSH/local session——AI 面板要求已连接的终端做诊断对象。
    // 没连接就让按钮 disabled，避免点了没反应（aiVisible 在 AppShell 层会因 session 缺失静默不渲染）。
    let canOpenAi = $derived.by(() => {
        const tab = app.activeTab();
        if (!tab || !app.isAiCapableTabType(tab.type)) return false;
        return !!app.sessionIdForTab(tab.id);
    });
    let aiOpen = $derived(ai.isOpen(app.activeTabId()));

    // 移动端唤起 AI 时提示一次：建议横屏 + 两个工具不可用。
    // 模块级 flag——一次 app run 提一次；togglePanel 只有"开"动作时才提。
    let mobileHintShown = false;
    function toggleAi() {
        if (!aiOpen && !canOpenAi) {
            toast.info(t("ai.no_session"));
            return;
        }
        if (!aiOpen && !mobileHintShown) {
            toast.info(t("ai.mobile.hint"));
            mobileHintShown = true;
        }
        void ai.togglePanel(app.activeTabId()).catch((e) => {
            console.warn("[ai] toggle mobile panel:", e);
            toast.error(errMsg(e));
        });
    }

    // 移动端唤起 SFTP 时提示一次：建议横屏。与 AI 面板同款，一次 app run 提一次。
    let sftpHintShown = false;
    function openSftpPanel() {
        if (!sftpHintShown) {
            toast.info(t("sftp.mobile.hint"));
            sftpHintShown = true;
        }
        app.openSftp();
    }
</script>

<div class="keybar">
    <button class="key mod" class:active={app.ctrlActive()} class:locked={app.ctrlLocked()}
        onpointerdown={(e) => { prevent(e); armLongPress("ctrl", () => app.lockCtrl()); }}
        onpointerup={() => finishPress("ctrl", () => app.setCtrl(!app.ctrlActive()))}
        onpointerleave={() => clearModTimer("ctrl")}
        onpointercancel={() => cancelPress("ctrl")}>Ctrl</button>
    <button class="key mod" class:active={app.altActive()} class:locked={app.altLocked()}
        onpointerdown={(e) => { prevent(e); armLongPress("alt", () => app.lockAlt()); }}
        onpointerup={() => finishPress("alt", () => app.setAlt(!app.altActive()))}
        onpointerleave={() => clearModTimer("alt")}
        onpointercancel={() => cancelPress("alt")}>Alt</button>
    <button class="key" onpointerdown={prevent} onclick={() => send('\x1b')}>Esc</button>
    <button class="key" onpointerdown={prevent} onclick={() => send('\t')}>Tab</button>
    <button class="key" onpointerdown={prevent} onclick={() => arrow('A')}>↑</button>
    <button class="key" onpointerdown={prevent} onclick={() => arrow('B')}>↓</button>
    <button class="key" onpointerdown={prevent} onclick={() => arrow('D')}>←</button>
    <button class="key" onpointerdown={prevent} onclick={() => arrow('C')}>→</button>
    <button class="key" class:active={extOpen} title="More keys" aria-label="More keys" onpointerdown={prevent} onclick={() => extOpen = !extOpen}>⋯</button>
    {#if extOpen}
        <!-- Transparent: a tap on the terminal area above dismisses the panel. -->
        <div class="ext-backdrop" onpointerdown={() => { extOpen = false; }}></div>
        <div class="ext-panel">
            {#each EXT_KEYS as k}
                <button class="key ext" onpointerdown={prevent} onclick={() => send(k.seq)}>{k.label}</button>
            {/each}
            <!-- Keys keep the panel open (you may press several); panel actions
               navigate away, so they dismiss it. -->
            <button class="key ext" title="Snippets" aria-label="Snippets" onpointerdown={prevent} onclick={() => { extOpen = false; app.openSnippetPicker(); }}>
                <AppIcon name="snippet" size={16} />
            </button>
            {#if app.activeTab()?.type === "ssh"}
                <button class="key ext" title="SFTP" aria-label="SFTP" onpointerdown={prevent} onclick={() => { extOpen = false; openSftpPanel(); }}>
                    <AppIcon name="folder" size={16} />
                </button>
            {/if}
            <button class="key ext" class:active={aiOpen} class:dim={!aiOpen && !canOpenAi} title="AI Chat" onpointerdown={prevent} onclick={() => { extOpen = false; toggleAi(); }}>AI</button>
        </div>
    {/if}
</div>

<style>
    .keybar {
        display: flex;
        gap: 4px;
        padding: 6px 8px;
        background: var(--bg);
        border-top: 1px solid var(--divider);
        flex-shrink: 0;
        position: relative; /* anchor for the floating extension panel + backdrop */
    }
    .key {
        flex: 1;
        height: 36px;
        border: none;
        border-radius: 6px;
        background: var(--surface);
        color: var(--text-sub);
        font-family: inherit;
        font-size: 13px;
        font-weight: 600;
        cursor: pointer;
        -webkit-tap-highlight-color: transparent;
        user-select: none;
    }
    .key :global(svg) { margin: auto; }
    .key:active {
        background: var(--divider);
    }
    /* Accent fill for any toggled-on key: armed modifiers, the open "..." and
       AI buttons. (AI's class:active binding predates this rule — it was a dead
       binding until this selector stopped requiring .mod.) */
    .key.active {
        background: var(--accent);
        color: var(--white);
    }
    /* Locked (long-press) vs one-shot (tap): both accent-filled, but locked gets
       an inset ring so you can tell at a glance that Ctrl will keep applying. */
    .key.mod.locked {
        box-shadow: inset 0 0 0 2px var(--white);
    }
    .key.dim { opacity: 0.45; }

    /* Floating extension panel: sits just above the bar, overlays the terminal
       instead of pushing it (the soft keyboard already eats half the screen).
       The backdrop covers everything above the bar so a tap there dismisses it. */
    .ext-backdrop {
        position: absolute;
        bottom: 100%;
        left: 0;
        right: 0;
        height: 100vh;
    }
    .ext-panel {
        position: absolute;
        bottom: 100%;
        left: 0;
        right: 0;
        display: flex;
        flex-wrap: wrap;
        gap: 4px;
        padding: 6px 8px;
        background: var(--bg);
        border-top: 1px solid var(--divider);
        border-radius: 8px 8px 0 0;
        z-index: 1; /* above the backdrop so the keys stay tappable */
    }
    .ext-panel .key.ext {
        flex: 0 0 auto;
        min-width: 44px;
    }
</style>
