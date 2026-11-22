<script module lang="ts">
    export interface CtxMenuItem {
        label: string;
        shortcut?: string;
        onClick: () => void;
        disabled?: boolean;
        /** Optional flyout children. Items without it render exactly as before. */
        submenu?: CtxMenuItem[];
    }
</script>

<script lang="ts">
    import {onMount, tick} from "svelte";
    import { createScopedBackHandler } from "../stores/back.svelte.ts";

    const registerBack = createScopedBackHandler();

    let {x, y, sections, onClose}: {
        x: number;
        y: number;
        sections: CtxMenuItem[][];
        onClose: () => void;
    } = $props();

    onMount(() => registerBack({ handle: () => onClose() }));

    let menuEl: HTMLDivElement | undefined;
    let dx = $state(0);
    let dy = $state(0);
    let ready = $state(false);

    onMount(() => {
        if (!menuEl) return;
        async function placeMenu() {
            const rect = menuEl!.getBoundingClientRect();
            dx = Math.max(4, Math.min(x, window.innerWidth - rect.width - 4)) - x;
            dy = Math.max(4, Math.min(y, window.innerHeight - rect.height - 4)) - y;
            await tick();
            placeOpenSubmenus();
        }
        placeMenu();
        const observer = new ResizeObserver(placeMenu);
        observer.observe(menuEl);
        window.addEventListener("resize", placeMenu);
        ready = true;
        return () => {
            observer.disconnect();
            window.removeEventListener("resize", placeMenu);
        };
    });

    function placeOpenSubmenus() {
        menuEl?.querySelectorAll<HTMLElement>(".ctx-sub-wrap:hover, .ctx-sub-wrap:focus-within").forEach(placeSubmenu);
    }

    function placeSubmenu(wrap: HTMLElement) {
        const flyout = wrap.querySelector<HTMLElement>(".ctx-submenu");
        if (!flyout) return;
        const anchor = wrap.getBoundingClientRect();
        const width = flyout.offsetWidth;
        const height = flyout.scrollHeight;
        const rightSpace = window.innerWidth - anchor.right - 4;
        const leftSpace = anchor.left - 4;
        const beside = width <= Math.max(rightSpace, leftSpace);
        let left = anchor.left;
        let top = anchor.bottom;
        let availableHeight = window.innerHeight - 8;
        if (beside) {
            left = rightSpace >= width ? anchor.right : anchor.left - width;
            top = Math.max(4, Math.min(anchor.top, window.innerHeight - height - 4));
        } else {
            // On narrow windows leave the clickable parent uncovered.
            left = Math.max(4, Math.min(left, window.innerWidth - width - 4));
            const below = window.innerHeight - anchor.bottom - 4;
            const above = anchor.top - 4;
            availableHeight = Math.max(above, below);
            if (above > below) top = anchor.top - Math.min(height, above);
        }
        flyout.style.left = left + "px";
        flyout.style.top = top + "px";
        flyout.style.maxHeight = availableHeight + "px";
    }

    function handleKeydown(e: KeyboardEvent) {
        if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            onClose();
        }
    }

    function handleClick(item: CtxMenuItem) {
        if (item.disabled) return;
        item.onClick();
        onClose();
    }
</script>

<svelte:window onkeydown={handleKeydown}/>

<!-- backdrop = 纯遮罩，键盘用户走顶层 Escape（handleKeydown）退出，不需要键盘 click 等价物 -->
<div class="ctx-backdrop"
     onclick={onClose}
     oncontextmenu={(e) => { e.preventDefault(); onClose(); }}
     role="presentation"></div>

<!-- 故意不用 role="menu" / "menuitem"——那两个 role 暗示完整 ARIA menu pattern
     （方向键导航 + roving tabindex），但本组件只有 Esc 关闭，假装是 menu 会让屏幕阅读器
     用户期待方向键导航却得不到。承认它就是一组上下文按钮：<button> 默认就是 button 角色，
     Tab 键可遍历、Enter/Space 可激活，简单诚实。 -->
<div class="ctx-menu surface-menu"
     class:ready
     bind:this={menuEl}
     onscroll={placeOpenSubmenus}
     style="left: {x + dx}px; top: {y + dy}px;">
    {#each sections as section, si (si)}
        {#if si > 0}<div class="ctx-sep"></div>{/if}
        {#each section as item, ii (ii)}
            {#if item.submenu}
                <!-- Parent stays clickable (= plain new window); hover/focus reveals
                     the flyout. Flyout is a SIBLING, not a child — <button> can't nest. -->
                <div class="ctx-sub-wrap" role="group"
                     onpointerenter={(event) => placeSubmenu(event.currentTarget)}
                     onfocusin={(event) => placeSubmenu(event.currentTarget)}>
                    <button class="ctx-item"
                            title={item.label}
                            class:disabled={item.disabled}
                            disabled={item.disabled}
                            onclick={() => handleClick(item)}>
                        <span class="ctx-label">{item.label}</span>
                        {#if item.shortcut}<span class="ctx-shortcut">{item.shortcut}</span>{/if}
                        <span class="ctx-caret" aria-hidden="true">›</span>
                    </button>
                    <div class="ctx-submenu surface-menu">
                        {#each item.submenu ?? [] as sub, ki (ki)}
                            <button class="ctx-item"
                                    title={sub.label}
                                    class:disabled={sub.disabled}
                                    disabled={sub.disabled}
                                    onclick={() => handleClick(sub)}>
                                <span class="ctx-label">{sub.label}</span>
                                {#if sub.shortcut}<span class="ctx-shortcut">{sub.shortcut}</span>{/if}
                            </button>
                        {/each}
                    </div>
                </div>
            {:else}
                <button class="ctx-item"
                        title={item.label}
                        class:disabled={item.disabled}
                        disabled={item.disabled}
                        onclick={() => handleClick(item)}>
                    <span class="ctx-label">{item.label}</span>
                    {#if item.shortcut}<span class="ctx-shortcut">{item.shortcut}</span>{/if}
                </button>
            {/if}
        {/each}
    {/each}
</div>

<style>
    .ctx-backdrop {
        position: fixed;
        inset: 0;
        z-index: 500;
    }
    .ctx-menu {
        position: fixed;
        z-index: 501;
        width: max-content;
        min-width: min(200px, calc(100vw - 8px));
        max-width: min(420px, calc(100vw - 8px));
        max-height: calc(100dvh - 8px);
        overflow-y: auto;
        padding: calc(4px * var(--density));
        display: flex;
        flex-direction: column;
        gap: 1px;
        /* Hidden until positioned to avoid flash off-screen */
        visibility: hidden;
    }
    .ctx-menu.ready {
        visibility: visible;
    }
    .ctx-item {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 20px;
        padding: 7px 12px;
        border: none;
        border-radius: var(--radius-sm);
        background: transparent;
        color: var(--text);
        font-family: inherit;
        font-size: 13px;
        text-align: left;
        cursor: pointer;
    }
    .ctx-item:hover:not(:disabled) {
        background: color-mix(in srgb, var(--text) 8%, transparent);
    }
    .ctx-item:disabled,
    .ctx-item.disabled {
        color: var(--text-dim);
        cursor: not-allowed;
    }
    .ctx-label {
        flex: 1;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .ctx-shortcut {
        color: var(--text-dim);
        font-size: 11px;
        font-family: var(--term-font);
        letter-spacing: 0.5px;
    }
    .ctx-sep {
        height: 1px;
        background: var(--divider);
        margin: 4px 6px;
    }
    .ctx-sub-wrap {
        position: relative;
        display: flex;
        flex-direction: column;
    }
    .ctx-caret {
        color: var(--text-dim);
        font-size: 14px;
        line-height: 1;
    }
    /* Flyout abuts the menu edge (no gap) so the pointer never crosses dead
       space on its way in — otherwise :hover drops and the submenu closes. */
    .ctx-submenu {
        position: fixed;
        z-index: 502;
        width: max-content;
        min-width: min(140px, calc(100vw - 8px));
        max-width: min(320px, calc(100vw - 8px));
        overflow-y: auto;
        padding: calc(4px * var(--density));
        visibility: hidden;
        display: flex;
        flex-direction: column;
        gap: 1px;
    }
    .ctx-sub-wrap:hover > .ctx-submenu,
    .ctx-sub-wrap:focus-within > .ctx-submenu {
        visibility: visible;
    }
</style>
