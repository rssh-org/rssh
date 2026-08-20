/*
 * docs/scenes.js — self-contained "mock rssh" demo scenes for the site.
 *
 * Ported 1:1 from the welcome screen (src/lib/components/welcome/Scene*.svelte):
 * same markup, same CSS, same beat schedules — driven by a tiny vanilla
 * player instead of Svelte state. Each scene mounts into its placeholder:
 *
 *     <figure class="rv" data-scene="ai|blocks|discovery|sync|cli"></figure>
 *
 * Scenes play while visible, reset when scrolled away, loop with a short
 * hold, and settle to their final state under prefers-reduced-motion.
 * The palette is scoped to .rssh-scene so host-page variables are untouched.
 */
(function () {
    'use strict';

    const CSS = `
        .rssh-scene {
            --bg: #2B2D3A;
            --surface: #32343F;
            --shadow-dark: #14161E;
            --text: #E0E5EC;
            --text-sub: #A0A8BB;
            --text-dim: #6B7A99;
            --accent: #4A6CF7;
            --accent-soft: rgba(74, 108, 247, 0.15);
            --success: #4CB88A;
            --warning: #DDAA33;
            --purple: #A855F7;
            --white: #FFFFFF;
            position: relative;
            width: min(1020px, 92vw);
            margin: clamp(34px, 4.2vw, 50px) auto 0;
            color: var(--text);
            text-align: left;
        }

        .rssh-scene .stage {
            position: relative;
            width: 100%;
            aspect-ratio: 16 / 10;
            max-height: 64vh;
        }

        .rssh-scene .mock-app {
            background: #1c1d24;
            border-radius: 14px;
            overflow: hidden;
            display: flex;
            flex-direction: column;
            box-shadow:
                0 30px 80px rgba(0, 0, 0, 0.6),
                0 0 0 1px rgba(255, 255, 255, 0.05);
        }

        /* Single-window scenes fill the stage; cli's two windows are grid
           children of its stage and must NOT be absolutely positioned. */
        .sc-ai .mock-app,
        .sc-blocks .mock-app,
        .sc-discovery .mock-app {
            position: absolute;
            inset: 0;
        }

        .rssh-scene .app-header {
            display: grid;
            grid-template-columns: 1fr auto 1fr;
            align-items: center;
            padding: 10px 14px;
            background: linear-gradient(180deg, #2a2c36 0%, #232530 100%);
            border-bottom: 1px solid rgba(0, 0, 0, 0.4);
            user-select: none;
            flex-shrink: 0;
        }
        .rssh-scene .dots { display: flex; gap: 7px; justify-self: start; }
        .rssh-scene .dot { width: 12px; height: 12px; border-radius: 50%; }
        .rssh-scene .dot.r { background: #ff5f57; }
        .rssh-scene .dot.y { background: #febc2e; }
        .rssh-scene .dot.g { background: #28c840; }
        .rssh-scene .app-title {
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 11px;
            color: rgba(255, 255, 255, 0.55);
            letter-spacing: 0.6px;
        }
        .rssh-scene .header-spacer { justify-self: end; }

        /* Stroke-only icons (AppIcon recipe): the sprite ships bare paths,
           so paint them here — default SVG fill:black would be invisible
           on the dark windows. */
        .rssh-scene .ic {
            display: block;
            flex: none;
            fill: none;
            stroke: currentColor;
            stroke-width: 1.8;
            stroke-linecap: round;
            stroke-linejoin: round;
        }

        /* MockCursor */
        .rssh-scene .mock-cursor {
            position: absolute;
            width: 22px;
            height: 24px;
            pointer-events: none;
            z-index: 50;
            opacity: 0;
            margin-left: -2px;
            margin-top: -2px;
            transition-property: left, top, opacity;
            transition-duration: 1000ms;
            transition-timing-function: cubic-bezier(0.22, 1, 0.36, 1);
            filter: drop-shadow(0 2px 4px rgba(0, 0, 0, 0.4));
            will-change: left, top;
        }
        .rssh-scene .mock-cursor.visible { opacity: 1; }
        .rssh-scene .mock-cursor .ripple {
            position: absolute;
            left: 2px;
            top: 2px;
            width: 12px;
            height: 12px;
            border-radius: 50%;
            background: color-mix(in srgb, var(--accent) 70%, transparent);
            display: none;
            pointer-events: none;
        }
        .rssh-scene .mock-cursor.clicking .ripple {
            display: block;
            animation: ripple 520ms ease-out forwards;
        }
        @keyframes ripple {
            0%   { transform: scale(0.4); opacity: 0.9; }
            100% { transform: scale(3.5); opacity: 0; }
        }

        @keyframes blink { 50% { opacity: 0; } }

        /* ── scene: AI ─────────────────────────────────────────────── */

        .sc-ai .ai-btn {
            justify-self: end;
            background: rgba(168, 85, 247, 0.12);
            border: 1px solid color-mix(in srgb, var(--purple) 50%, transparent);
            color: var(--purple);
            font-family: inherit;
            font-size: 11px;
            font-weight: 700;
            letter-spacing: 0.8px;
            padding: 5px 10px;
            border-radius: 6px;
            cursor: default;
            display: inline-flex;
            align-items: center;
            gap: 5px;
            transition: background 0.2s ease, box-shadow 0.2s ease, transform 0.12s ease;
        }
        .sc-ai.st-open .ai-btn {
            background: color-mix(in srgb, var(--purple) 30%, transparent);
            box-shadow: 0 0 0 3px color-mix(in srgb, var(--purple) 20%, transparent);
        }

        .sc-ai .app-body {
            flex: 1;
            display: flex;
            position: relative;
            overflow: hidden;
        }
        .sc-ai .term-pane {
            flex: 1;
            padding: 16px 18px;
            font-family: "SF Mono", Menlo, Consolas, "Courier New", monospace;
            font-size: 13px;
            line-height: 1.65;
            color: #d4d8e2;
            min-width: 0;
        }
        .sc-ai .ln { white-space: pre-wrap; }
        .sc-ai .ln.out { color: #b8bcc8; }
        .sc-ai .ln.out.warn { color: #f0c674; }
        .sc-ai .ln.app { display: none; }
        .sc-ai.st-approved .ln.app { display: block; animation: ln-app-in 320ms ease both; }
        @keyframes ln-app-in { from { opacity: 0; transform: translateY(3px); } to { opacity: 1; transform: translateY(0); } }
        .sc-ai .ln .hot { color: #ff6b6b; font-weight: 700; }
        .sc-ai .prompt { color: var(--success); margin-right: 6px; }
        .sc-ai .cur-blink { color: var(--accent); animation: blink 1s steps(1, start) infinite; }

        .sc-ai .ai-pane {
            position: absolute;
            top: 0; bottom: 0; right: 0;
            width: 42%;
            background: linear-gradient(180deg, #20212a 0%, #1c1d24 100%);
            border-left: 1px solid color-mix(in srgb, var(--purple) 25%, transparent);
            display: flex;
            flex-direction: column;
            transform: translateX(100%);
            transition: transform 650ms cubic-bezier(0.22, 1, 0.36, 1), box-shadow 500ms ease;
            z-index: 2;
        }
        .sc-ai.st-open .ai-pane {
            transform: translateX(0);
            box-shadow: -20px 0 60px rgba(0, 0, 0, 0.5);
        }
        .sc-ai.st-focus .ai-pane {
            box-shadow:
                -28px 0 80px rgba(0, 0, 0, 0.65),
                0 0 0 1px color-mix(in srgb, var(--purple) 35%, transparent);
        }

        .sc-ai .ai-head {
            display: flex;
            align-items: center;
            gap: 8px;
            padding: 12px 14px;
            border-bottom: 1px solid rgba(255, 255, 255, 0.06);
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 11px;
        }
        .sc-ai .ai-dot {
            width: 7px; height: 7px; border-radius: 50%;
            background: var(--purple);
            box-shadow: 0 0 8px var(--purple);
        }
        .sc-ai .ai-name { color: var(--text); font-weight: 700; letter-spacing: 0.5px; }
        .sc-ai .ai-model { margin-left: auto; color: var(--text-dim); font-size: 10px; letter-spacing: 0.5px; }

        .sc-ai .ai-thread {
            flex: 1;
            padding: 14px;
            overflow: hidden;
            display: flex;
            flex-direction: column;
            gap: 10px;
        }

        .sc-ai .bubble {
            display: none;
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 12px;
            padding: 8px 12px;
            border-radius: 12px;
            max-width: 100%;
            line-height: 1.45;
            animation: bubble-in 360ms cubic-bezier(0.22, 1, 0.36, 1) both;
        }
        .sc-ai.st-sent .bubble.user,
        .sc-ai.st-sent .bubble.dots-bubble { display: flex; }
        .sc-ai.st-reply .bubble.dots-bubble { display: none; }
        .sc-ai.st-reply .bubble.reply-bubble { display: flex; }
        .sc-ai.st-tool .bubble.card-bubble { display: flex; }
        .sc-ai .bubble.reply-bubble { color: var(--text-sub); font-size: 11px; line-height: 1.5; }
        .sc-ai .bubble.user {
            align-self: flex-end;
            background: color-mix(in srgb, var(--accent) 25%, transparent);
            color: var(--text);
            border: 1px solid color-mix(in srgb, var(--accent) 35%, transparent);
        }
        .sc-ai .bubble.asst {
            align-self: flex-start;
            background: rgba(255, 255, 255, 0.04);
            border: 1px solid rgba(255, 255, 255, 0.06);
            color: var(--text);
            animation-delay: 200ms;
            flex-direction: column;
            gap: 8px;
        }
        @keyframes bubble-in { from { opacity: 0; transform: translateY(6px); } to { opacity: 1; transform: translateY(0); } }

        .sc-ai .asst-line { color: var(--text-sub); font-size: 11px; line-height: 1.5; }

        .sc-ai .dots-anim { display: none; gap: 4px; padding: 4px 0; }
        .sc-ai.st-sent:not(.st-reply) .dots-anim { display: inline-flex; }
        .sc-ai .dots-anim span {
            width: 5px; height: 5px;
            border-radius: 50%;
            background: var(--text-dim);
            animation: dotbounce 1.1s ease-in-out infinite;
        }
        .sc-ai .dots-anim span:nth-child(2) { animation-delay: 0.18s; }
        .sc-ai .dots-anim span:nth-child(3) { animation-delay: 0.36s; }
        @keyframes dotbounce {
            0%, 80%, 100% { transform: translateY(0); opacity: 0.4; }
            40%           { transform: translateY(-4px); opacity: 1; }
        }

        .sc-ai .tool-card {
            display: none;
            background: rgba(168, 85, 247, 0.08);
            border: 1px solid color-mix(in srgb, var(--purple) 35%, transparent);
            border-radius: 8px;
            padding: 8px 10px;
            font-size: 11px;
            flex-direction: column;
            gap: 6px;
            animation: tool-in 380ms cubic-bezier(0.22, 1, 0.36, 1) 300ms both;
        }
        .sc-ai.st-tool .tool-card { display: flex; }
        .sc-ai.st-approved .tool-card {
            background: color-mix(in srgb, var(--success) 8%, rgba(168, 85, 247, 0.06));
            border-color: color-mix(in srgb, var(--success) 40%, transparent);
        }
        @keyframes tool-in { from { opacity: 0; transform: translateY(4px) scale(0.97); } to { opacity: 1; transform: translateY(0) scale(1); } }
        .sc-ai .tool-head { display: flex; align-items: center; gap: 8px; }
        .sc-ai .tool-tag {
            background: var(--purple);
            color: var(--white);
            padding: 1px 6px;
            border-radius: 4px;
            font-size: 9px;
            letter-spacing: 0.8px;
            font-weight: 700;
            text-transform: uppercase;
        }
        .sc-ai .tool-name { color: var(--text); font-weight: 700; }
        .sc-ai .tool-status {
            display: none;
            margin-left: auto;
            color: var(--success);
            font-size: 10px;
            font-weight: 700;
            letter-spacing: 0.5px;
            align-items: center;
            gap: 4px;
        }
        .sc-ai.st-approved .tool-status { display: inline-flex; }
        .sc-ai .tool-args {
            color: var(--text-sub);
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 10px;
            background: rgba(0, 0, 0, 0.25);
            padding: 4px 8px;
            border-radius: 4px;
        }
        .sc-ai .tool-guard {
            display: flex;
            gap: 10px;
            font-size: 9.5px;
            color: var(--text-dim);
            letter-spacing: 0.4px;
        }
        .sc-ai .guard-item { display: inline-flex; align-items: center; gap: 3px; }

        .sc-ai .tool-actions { display: flex; justify-content: flex-end; }
        .sc-ai.st-approved .tool-actions { display: none; }
        .sc-ai .approve-btn {
            font-family: inherit;
            font-size: 10.5px;
            font-weight: 700;
            letter-spacing: 0.5px;
            color: var(--success);
            background: color-mix(in srgb, var(--success) 12%, transparent);
            border: 1px solid color-mix(in srgb, var(--success) 45%, transparent);
            border-radius: 5px;
            padding: 4px 14px;
            cursor: default;
            display: inline-flex;
            align-items: center;
            transition: transform 150ms ease, box-shadow 150ms ease;
        }
        /* Click feedback: same halo language as the focused input box. */
        .sc-ai.st-press .approve-btn {
            transform: scale(0.94);
            box-shadow:
                0 0 0 4px color-mix(in srgb, var(--success) 22%, transparent),
                0 10px 28px color-mix(in srgb, var(--success) 30%, transparent);
        }

        .sc-ai .ai-input {
            margin: 0 12px 12px;
            padding: 10px 12px;
            border-radius: 10px;
            background: rgba(0, 0, 0, 0.35);
            border: 1px solid rgba(255, 255, 255, 0.07);
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 12px;
            display: flex;
            align-items: center;
            gap: 6px;
            transition: transform 360ms cubic-bezier(0.22, 1, 0.36, 1),
                        box-shadow 360ms ease,
                        border-color 360ms ease;
            transform-origin: bottom center;
        }
        /* Glow persists once focused — the panel stays lit through approve. */
        .sc-ai.st-focus .ai-input {
            transform: scale(1.08);
            border-color: color-mix(in srgb, var(--purple) 60%, transparent);
            box-shadow:
                0 0 0 4px color-mix(in srgb, var(--purple) 20%, transparent),
                0 14px 36px rgba(0, 0, 0, 0.5);
        }
        .sc-ai .caret-bar { color: var(--purple); font-weight: 700; }
        .sc-ai .typed { color: var(--text); }
        .sc-ai .caret-blink { display: none; color: var(--purple); animation: blink 1s steps(1, start) infinite; font-weight: 700; }
        .sc-ai:not(.st-sent) .caret-blink { display: inline; }
        .sc-ai .enter-key {
            margin-left: auto;
            width: 24px;
            height: 20px;
            border-radius: 4px;
            background: rgba(255, 255, 255, 0.06);
            color: var(--text-dim);
            display: inline-flex;
            align-items: center;
            justify-content: center;
            font-size: 11px;
            border: 1px solid rgba(255, 255, 255, 0.08);
            transition: background 0.18s ease, color 0.18s ease, box-shadow 0.18s ease;
        }
        .sc-ai.st-sent .enter-key {
            background: var(--accent);
            color: var(--white);
            box-shadow: 0 0 0 4px color-mix(in srgb, var(--accent) 20%, transparent);
        }

        /* ── scene: blocks ─────────────────────────────────────────── */

        .sc-blocks .term-pane {
            flex: 1;
            padding: 18px 22px;
            font-family: "SF Mono", Menlo, Consolas, "Courier New", monospace;
            font-size: 13px;
            line-height: 1.6;
            color: #d4d8e2;
            overflow: hidden;
            display: flex;
            flex-direction: column;
            gap: 12px;
        }

        .sc-blocks .block {
            position: relative;
            display: flex;
            gap: 14px;
            padding: 6px 4px 6px 0;
            border-radius: 6px;
            transition:
                background 300ms ease,
                transform 360ms cubic-bezier(0.22, 1, 0.36, 1),
                filter 360ms ease,
                opacity 360ms ease;
        }
        .sc-blocks .bar {
            width: 3px;
            background: rgba(255, 255, 255, 0.08);
            border-radius: 0 2px 2px 0;
            flex-shrink: 0;
            transition: background 400ms ease, box-shadow 400ms ease;
        }
        .sc-blocks .block.lit .bar {
            background: var(--bar);
            box-shadow: 0 0 10px color-mix(in srgb, var(--bar) 50%, transparent);
        }
        .sc-blocks .block-body { flex: 1; min-width: 0; }
        .sc-blocks .ln {
            white-space: pre-wrap;
            max-height: 36px;
            overflow: hidden;
            transition: max-height 280ms ease, opacity 220ms ease, margin 220ms ease;
        }
        .sc-blocks .ln.out { color: #b8bcc8; }
        .sc-blocks .ln.cmd-ln { color: var(--text); }
        .sc-blocks .ln.folded-hint {
            display: none;
            color: var(--text-dim);
            font-style: italic;
            font-size: 12px;
        }
        .sc-blocks .block.folded .folded-hint {
            display: block;
            animation: fold-hint-in 320ms 240ms forwards;
            opacity: 0;
        }
        @keyframes fold-hint-in { to { opacity: 1; } }
        .sc-blocks .prompt { color: var(--success); margin-right: 6px; }

        .sc-blocks .block.folded .ln.out {
            max-height: 0;
            opacity: 0;
            margin: 0;
        }

        .sc-blocks .block.selected {
            background: color-mix(in srgb, var(--bar) 14%, transparent);
        }
        .sc-blocks .block.selected .bar {
            width: 4px;
            box-shadow: 0 0 14px color-mix(in srgb, var(--bar) 80%, transparent);
        }
        .sc-blocks .block.selected .ln {
            background: color-mix(in srgb, var(--bar) 22%, transparent);
            margin: 0 -4px;
            padding: 0 4px;
        }

        .sc-blocks .block.dim {
            filter: brightness(0.4) saturate(0.6);
            opacity: 0.6;
        }
        .sc-blocks.st-focus .block.selected {
            transform: scale(1.04);
            transform-origin: left center;
            z-index: 2;
        }

        .sc-blocks .ctx-menu {
            position: absolute;
            left: 6%;
            display: none;
            background: #232530;
            border: 1px solid rgba(255, 255, 255, 0.08);
            border-radius: 8px;
            padding: 4px;
            min-width: 210px;
            box-shadow: 0 16px 40px rgba(0, 0, 0, 0.55);
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 12px;
            color: var(--text);
            z-index: 40;
        }
        .sc-blocks .ctx-menu.open {
            display: block;
            animation: menu-in 200ms cubic-bezier(0.22, 1, 0.36, 1) forwards;
            opacity: 0;
        }
        @keyframes menu-in {
            from { opacity: 0; transform: translateY(4px); }
            to   { opacity: 1; transform: translateY(0); }
        }
        .sc-blocks .ctx-item {
            display: flex;
            align-items: center;
            gap: 8px;
            padding: 6px 12px;
            border-radius: 5px;
            transition: background 0.15s ease;
        }
        .sc-blocks .ctx-item.hovered {
            background: color-mix(in srgb, var(--accent) 22%, transparent);
        }
        .sc-blocks .ctx-label { color: var(--text); }
        .sc-blocks .ctx-key { font-size: 10px; color: var(--text-dim); letter-spacing: 0.4px; }
        .sc-blocks .ctx-sep { height: 1px; background: rgba(255, 255, 255, 0.06); margin: 3px 4px; }

        .sc-blocks .burst-host { position: absolute; pointer-events: none; z-index: 50; display: none; }
        .sc-blocks .burst-host.on { display: block; }
        .sc-blocks .thumb {
            position: absolute;
            top: 0;
            left: 0;
            width: clamp(80px, 8vw, 100px);
            height: clamp(52px, 6vw, 64px);
            background: linear-gradient(180deg, #2a2c36 0%, #1c1d24 100%);
            border: 1px solid color-mix(in srgb, var(--accent) 35%, transparent);
            border-radius: 5px;
            padding: 6px 8px 7px;
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 9px;
            color: var(--success);
            box-shadow: 0 14px 30px rgba(0, 0, 0, 0.6);
            opacity: 0;
            overflow: hidden;
            white-space: nowrap;
            text-overflow: ellipsis;
        }
        .sc-blocks .thumb::after {
            content: "";
            position: absolute;
            left: 8px;
            right: 8px;
            bottom: 7px;
            height: 22px;
            background:
                linear-gradient(180deg,
                    rgba(255, 255, 255, 0.16) 0 2px, transparent 2px 7px,
                    rgba(255, 255, 255, 0.12) 7px 9px, transparent 9px 14px,
                    rgba(255, 255, 255, 0.09) 14px 16px, transparent 16px 22px);
        }
        .sc-blocks .thumb.t1 { animation: burst-1 900ms cubic-bezier(0.22, 1, 0.36, 1) forwards; }
        .sc-blocks .thumb.t2 { animation: burst-2 950ms 50ms cubic-bezier(0.22, 1, 0.36, 1) forwards; }
        .sc-blocks .thumb.t3 { animation: burst-3 1000ms 100ms cubic-bezier(0.22, 1, 0.36, 1) forwards; }
        .sc-blocks .thumb.t4 { animation: burst-4 1050ms 150ms cubic-bezier(0.22, 1, 0.36, 1) forwards; }
        @keyframes burst-1 {
            0%   { opacity: 0; transform: translate(-50%, -50%) scale(0.6) rotate(0deg); }
            25%  { opacity: 1; transform: translate(-50%, -50%) scale(1) rotate(-2deg); }
            100% { opacity: 0; transform: translate(-200%, -140%) scale(0.88) rotate(-22deg); }
        }
        @keyframes burst-2 {
            0%   { opacity: 0; transform: translate(-50%, -50%) scale(0.6); }
            25%  { opacity: 1; transform: translate(-50%, -50%) scale(1) rotate(5deg); }
            100% { opacity: 0; transform: translate(80%, -160%) scale(0.94) rotate(18deg); }
        }
        @keyframes burst-3 {
            0%   { opacity: 0; transform: translate(-50%, -50%) scale(0.6); }
            25%  { opacity: 1; transform: translate(-50%, -50%) scale(1) rotate(-1deg); }
            100% { opacity: 0; transform: translate(-230%, 50%) scale(0.96) rotate(-28deg); }
        }
        @keyframes burst-4 {
            0%   { opacity: 0; transform: translate(-50%, -50%) scale(0.6); }
            25%  { opacity: 1; transform: translate(-50%, -50%) scale(1) rotate(3deg); }
            100% { opacity: 0; transform: translate(130%, 70%) scale(0.9) rotate(30deg); }
        }

        /* ── scene: discovery ──────────────────────────────────────── */

        .sc-discovery .stage { max-width: 980px; margin: 0 auto; }

        .sc-discovery .sources {
            display: flex;
            gap: 10px;
            padding: 10px 16px 0;
            opacity: 0;
            transform: translateY(-6px);
            transition: opacity 400ms ease, transform 400ms cubic-bezier(0.22, 1, 0.36, 1);
            flex-shrink: 0;
        }
        .sc-discovery.st-sources .sources { opacity: 1; transform: translateY(0); }
        .sc-discovery .source {
            display: inline-flex;
            align-items: center;
            gap: 7px;
            padding: 5px 11px;
            border: 1px solid rgba(255, 255, 255, 0.1);
            border-radius: 999px;
            background: rgba(255, 255, 255, 0.04);
            color: var(--text-sub);
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 11px;
            transition: border-color 0.3s ease, box-shadow 0.3s ease;
        }
        .sc-discovery .source .ic { color: var(--accent); }
        .sc-discovery .source:nth-child(2) .ic { color: var(--purple); }
        .sc-discovery.st-scan .source {
            border-color: color-mix(in srgb, var(--accent) 55%, transparent);
            box-shadow: 0 0 12px color-mix(in srgb, var(--accent) 25%, transparent);
        }

        .sc-discovery .home-list {
            position: relative;
            flex: 1;
            min-height: 0;
            padding: 8px 16px 8px;
            display: flex;
            flex-direction: column;
            justify-content: center;
            gap: clamp(6px, 1.5vh, 12px);
            overflow: hidden;
        }
        .sc-discovery .hsection.h-docker { display: none; }
        .sc-discovery.st-docker .hsection.h-docker { display: block; animation: section-in 420ms ease both; }
        .sc-discovery .hsection.h-k8s { display: none; }
        .sc-discovery.st-k8s .hsection.h-k8s { display: block; animation: section-in 420ms ease both; }
        @keyframes section-in {
            from { opacity: 0; transform: translateY(6px); }
            to   { opacity: 1; transform: translateY(0); }
        }
        .sc-discovery .hlabel {
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 10px;
            font-weight: 700;
            letter-spacing: 1.2px;
            text-transform: uppercase;
            color: var(--text-dim);
            margin-bottom: 5px;
        }
        .sc-discovery .hcards {
            display: grid;
            grid-template-columns: repeat(2, minmax(0, 1fr));
            gap: 8px;
        }

        .sc-discovery .tcard {
            display: flex;
            align-items: center;
            gap: 9px;
            padding: 8px 10px;
            border-radius: 9px;
            background: rgba(255, 255, 255, 0.04);
            border: 1px solid rgba(255, 255, 255, 0.05);
            min-width: 0;
            transition: box-shadow 0.25s ease;
        }
        .sc-discovery .tcard.pop { animation: card-pop 460ms cubic-bezier(0.22, 1, 0.36, 1) both; }
        .sc-discovery .tcard.pop.d2 { animation-delay: 120ms; }
        @keyframes card-pop {
            from { opacity: 0; transform: scale(0.85) translateY(8px); }
            to   { opacity: 1; transform: scale(1) translateY(0); }
        }
        .sc-discovery .tcard.flash {
            border-color: color-mix(in srgb, var(--accent) 60%, transparent);
            box-shadow:
                0 0 0 2px color-mix(in srgb, var(--accent) 45%, transparent),
                0 0 18px color-mix(in srgb, var(--accent) 30%, transparent);
            animation: card-flash 900ms ease-out both;
        }
        @keyframes card-flash {
            0% { transform: scale(0.97); }
            40% { transform: scale(1.04); }
            100% { transform: scale(1); }
        }

        .sc-discovery .ticon {
            width: 28px; height: 28px;
            border-radius: 8px;
            background: var(--accent-soft);
            color: var(--accent);
            display: inline-flex;
            align-items: center;
            justify-content: center;
            flex-shrink: 0;
        }
        .sc-discovery .ticon.k8s { background: color-mix(in srgb, var(--purple) 15%, transparent); color: var(--purple); }
        .sc-discovery .tbody { display: flex; flex-direction: column; min-width: 0; }
        .sc-discovery .tname {
            font-size: 12.5px;
            font-weight: 600;
            color: var(--text);
            white-space: nowrap;
            overflow: hidden;
            text-overflow: ellipsis;
        }
        .sc-discovery .tsub {
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 10.5px;
            color: var(--text-sub);
            white-space: nowrap;
            overflow: hidden;
            text-overflow: ellipsis;
        }

        .sc-discovery .scanline {
            position: absolute;
            left: 0;
            right: 0;
            top: 0;
            height: 2px;
            background: linear-gradient(90deg, transparent, var(--accent) 30%, var(--accent) 70%, transparent);
            box-shadow: 0 0 14px var(--accent);
            opacity: 0;
            pointer-events: none;
        }
        .sc-discovery.st-scan .scanline { animation: scan 1000ms cubic-bezier(0.4, 0, 0.6, 1) forwards; }
        @keyframes scan {
            0%   { top: 0; opacity: 0.9; }
            100% { top: 100%; opacity: 0; }
        }

        .sc-discovery .term-strip {
            display: none;
            flex-shrink: 0;
            margin: 0 16px 12px;
            padding: 9px 14px;
            border-radius: 9px;
            background: #14151b;
            border: 1px solid rgba(255, 255, 255, 0.06);
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 12.5px;
            line-height: 1.8;
            color: #d4d8e2;
        }
        .sc-discovery.st-open .term-strip { display: block; animation: term-in 420ms cubic-bezier(0.22, 1, 0.36, 1) both; }
        @keyframes term-in {
            from { opacity: 0; transform: translateY(10px); }
            to   { opacity: 1; transform: translateY(0); }
        }
        .sc-discovery .tln { white-space: pre; }
        .sc-discovery .ps1 { color: var(--success); font-weight: 700; margin-right: 6px; }
        .sc-discovery .root { color: var(--accent); font-weight: 700; margin-right: 6px; }
        .sc-discovery .caret { color: var(--accent); animation: blink 1s steps(1, start) infinite; font-weight: 700; }
        .sc-discovery .t-root { display: none; }
        .sc-discovery.st-typed .t-root { display: block; }

        /* ── scene: sync ───────────────────────────────────────────── */

        .sc-sync .lines {
            position: absolute;
            inset: 0;
            width: 100%;
            height: 100%;
            z-index: 1;
            pointer-events: none;
            overflow: visible;
        }
        .sc-sync .connect {
            stroke: #3c3f50;
            stroke-width: 2;
            stroke-dasharray: 6 4;
            stroke-dashoffset: 800;
            transition: stroke-dashoffset 700ms ease, stroke 400ms ease;
            fill: none;
        }
        .sc-sync.s-wired .c1, .sc-sync.s-wired .c2 { stroke-dashoffset: 0; stroke: var(--accent); }
        .sc-sync.s-encrypting .c3 { stroke-dashoffset: 0; stroke: var(--warning); }
        .sc-sync.s-fanout .c4 { stroke-dashoffset: 0; stroke: var(--success); }
        .sc-sync.s-fanout .c5 { stroke-dashoffset: 0; stroke: var(--success); transition-delay: 60ms; }
        .sc-sync.s-fanout .c6 { stroke-dashoffset: 0; stroke: var(--success); transition-delay: 120ms; }
        .sc-sync.s-fanout .c7 { stroke-dashoffset: 0; stroke: var(--success); transition-delay: 180ms; }

        .sc-sync .enc-label {
            fill: var(--warning);
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 12px;
            font-weight: 700;
            letter-spacing: 1px;
            text-anchor: middle;
            opacity: 0;
            transition: opacity 400ms 200ms ease;
        }
        .sc-sync.s-encrypting .enc-label, .sc-sync.s-stored .enc-label,
        .sc-sync.s-fanout .enc-label, .sc-sync.s-lit .enc-label {
            opacity: 1;
        }

        .sc-sync .node {
            position: absolute;
            display: flex;
            flex-direction: column;
            align-items: center;
            gap: 6px;
            padding: 12px 14px;
            background: #1c1d24;
            border: 1px solid rgba(255, 255, 255, 0.07);
            border-radius: 12px;
            color: var(--text);
            font-family: "SF Mono", Menlo, Consolas, monospace;
            opacity: 0;
            z-index: 2;
            box-shadow: 0 12px 32px rgba(0, 0, 0, 0.4);
            transition: opacity 400ms ease, box-shadow 400ms ease, transform 400ms ease;
            text-align: center;
        }
        .sc-sync.s-nodes .node { opacity: 1; }

        .sc-sync .rssh {
            left: 4%;
            top: 50%;
            transform: translate(0, -50%);
            color: var(--accent);
            width: 12%;
            min-width: 110px;
            box-shadow:
                0 12px 32px rgba(0, 0, 0, 0.4),
                0 0 0 1px color-mix(in srgb, var(--accent) 30%, transparent);
        }
        .sc-sync .rssh-icon {
            color: var(--accent);
            width: 44px;
            height: 44px;
            background: color-mix(in srgb, var(--accent) 14%, transparent);
            border: 1px solid color-mix(in srgb, var(--accent) 35%, transparent);
            border-radius: 8px;
        }
        .sc-sync .rssh-glyph {
            font-size: 28px;
            font-weight: 700;
            letter-spacing: -0.04em;
            color: var(--accent);
            text-shadow:
                0 0 8px color-mix(in srgb, var(--accent) 60%, transparent),
                0 0 18px color-mix(in srgb, var(--accent) 30%, transparent);
        }

        .sc-sync .keyserver {
            left: 22%;
            top: 25%;
            transform: translate(0, -50%);
            color: var(--warning);
            width: 16%;
            min-width: 140px;
            box-shadow:
                0 12px 32px rgba(0, 0, 0, 0.4),
                0 0 0 1px color-mix(in srgb, var(--warning) 35%, transparent),
                0 0 30px color-mix(in srgb, var(--warning) 15%, transparent);
        }
        .sc-sync .key-stay {
            position: absolute;
            right: -8px;
            bottom: -4px;
            background: rgba(28, 30, 42, 0.95);
            border: 1px solid color-mix(in srgb, var(--warning) 55%, transparent);
            border-radius: 6px;
            padding: 2px 4px;
            display: inline-flex;
            align-items: center;
            filter: drop-shadow(0 0 6px color-mix(in srgb, var(--warning) 60%, transparent));
        }

        .sc-sync .db {
            left: 22%;
            top: 75%;
            transform: translate(0, -50%);
            color: var(--accent);
            width: 16%;
            min-width: 140px;
        }

        .sc-sync .repo {
            left: 45%;
            top: 50%;
            transform: translate(0, -50%);
            color: var(--text-sub);
            width: 16%;
            min-width: 140px;
        }
        .sc-sync.s-stored .repo, .sc-sync.s-fanout .repo, .sc-sync.s-lit .repo {
            color: var(--success);
            box-shadow:
                0 12px 32px rgba(0, 0, 0, 0.4),
                0 0 0 1px color-mix(in srgb, var(--success) 35%, transparent),
                0 0 30px color-mix(in srgb, var(--success) 18%, transparent);
        }
        .sc-sync .repo-icon { position: relative; }
        .sc-sync .doc-stored {
            position: absolute;
            left: -4px;
            bottom: -2px;
            background: #1c1d24;
            border-radius: 4px;
            padding: 1px 2px;
            opacity: 0;
            transform: scale(0.6);
            transition: opacity 320ms ease, transform 320ms cubic-bezier(0.22, 1, 0.36, 1);
        }
        .sc-sync.s-stored .doc-stored, .sc-sync.s-fanout .doc-stored, .sc-sync.s-lit .doc-stored {
            opacity: 1;
            transform: scale(1);
        }
        .sc-sync .lock-badge {
            position: absolute;
            right: -10px;
            top: -8px;
            color: var(--success);
            opacity: 0;
            transform: scale(0.4);
            transition: opacity 300ms ease, transform 320ms cubic-bezier(0.22, 1, 0.36, 1);
            filter: drop-shadow(0 0 6px color-mix(in srgb, var(--success) 70%, transparent));
        }
        .sc-sync.s-stored .lock-badge, .sc-sync.s-fanout .lock-badge, .sc-sync.s-lit .lock-badge {
            opacity: 1;
            transform: scale(1);
        }

        .sc-sync .platform {
            right: 4%;
            width: 11%;
            min-width: 96px;
            padding: 6px 10px;
            flex-direction: row;
            align-items: center;
            justify-content: flex-start;
            gap: 10px;
            color: var(--pf);
        }
        .sc-sync.s-nodes .platform { opacity: 0.55; }
        .sc-sync.s-lit .platform {
            opacity: 1;
            box-shadow:
                0 12px 32px rgba(0, 0, 0, 0.4),
                0 0 0 1px color-mix(in srgb, var(--pf) 35%, transparent);
        }
        .sc-sync .pf-tile {
            width: 28px;
            height: 28px;
            border-radius: 6px;
            background: color-mix(in srgb, var(--pf) 18%, transparent);
            border: 1px solid color-mix(in srgb, var(--pf) 50%, transparent);
            color: var(--pf);
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 14px;
            font-weight: 700;
            display: inline-flex;
            align-items: center;
            justify-content: center;
            flex-shrink: 0;
        }
        .sc-sync .node-label-sm { font-size: 11px; color: var(--text); letter-spacing: 0.3px; }

        .sc-sync .p-win { top: 17%; transform: translate(0, -50%); }
        .sc-sync .p-mac { top: 37%; transform: translate(0, -50%); }
        .sc-sync .p-lin { top: 63%; transform: translate(0, -50%); }
        .sc-sync .p-and { top: 83%; transform: translate(0, -50%); }

        .sc-sync .node-icon {
            color: inherit;
            display: inline-flex;
            align-items: center;
            justify-content: center;
            position: relative;
        }
        .sc-sync .node-label {
            font-size: 11px;
            font-weight: 700;
            letter-spacing: 0.6px;
            color: var(--text);
            text-transform: uppercase;
        }
        .sc-sync .node-sub {
            font-size: 10px;
            color: var(--text-dim);
            letter-spacing: 0.3px;
            line-height: 1.4;
        }

        .sc-sync .doc {
            position: absolute;
            z-index: 3;
            opacity: 0;
            pointer-events: none;
            filter: drop-shadow(0 0 6px rgba(125, 211, 252, 0.6));
            will-change: left, top, opacity;
        }
        .sc-sync .doc.d-encrypt { left: 30%; top: 75%; transform: translate(-50%, -50%); }
        .sc-sync.s-encrypting .d-encrypt { animation: doc-encrypt 1300ms cubic-bezier(0.5, -0.1, 0.5, 1) forwards; }
        @keyframes doc-encrypt {
            0%   { opacity: 0; left: 30%; top: 75%; }
            12%  { opacity: 1; }
            50%  { left: 40%; top: 62%; }
            90%  { opacity: 1; }
            100% { opacity: 0; left: 53%; top: 50%; }
        }
        .sc-sync .doc.d-out-1, .sc-sync .doc.d-out-2,
        .sc-sync .doc.d-out-3, .sc-sync .doc.d-out-4 {
            left: 53%; top: 50%; transform: translate(-50%, -50%);
        }
        .sc-sync.s-fanout .d-out-1 { animation: doc-out-1 1100ms cubic-bezier(0.5, 0, 0.5, 1) 0ms forwards; }
        .sc-sync.s-fanout .d-out-2 { animation: doc-out-2 1100ms cubic-bezier(0.5, 0, 0.5, 1) 120ms forwards; }
        .sc-sync.s-fanout .d-out-3 { animation: doc-out-3 1100ms cubic-bezier(0.5, 0, 0.5, 1) 240ms forwards; }
        .sc-sync.s-fanout .d-out-4 { animation: doc-out-4 1100ms cubic-bezier(0.5, 0, 0.5, 1) 360ms forwards; }
        @keyframes doc-out-1 {
            0%   { opacity: 0; left: 53%; top: 50%; }
            10%  { opacity: 1; }
            100% { opacity: 0; left: 85%; top: 17%; }
        }
        @keyframes doc-out-2 {
            0%   { opacity: 0; left: 53%; top: 50%; }
            10%  { opacity: 1; }
            100% { opacity: 0; left: 85%; top: 37%; }
        }
        @keyframes doc-out-3 {
            0%   { opacity: 0; left: 53%; top: 50%; }
            10%  { opacity: 1; }
            100% { opacity: 0; left: 85%; top: 63%; }
        }
        @keyframes doc-out-4 {
            0%   { opacity: 0; left: 53%; top: 50%; }
            10%  { opacity: 1; }
            100% { opacity: 0; left: 85%; top: 83%; }
        }

        /* ── scene: cli ────────────────────────────────────────────── */

        .sc-cli .stage {
            display: grid;
            grid-template-columns: 1fr 1fr;
            gap: 24px;
        }

        .sc-cli .term-body {
            flex: 1;
            padding: 16px 18px;
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 13px;
            line-height: 1.7;
            color: #d4d8e2;
        }
        .sc-cli .ln { white-space: pre-wrap; }
        .sc-cli .ln.dim { color: var(--text-dim); }
        .sc-cli .ln.out { color: var(--success); }
        .sc-cli .hl {
            background: color-mix(in srgb, var(--success) 18%, transparent);
            color: var(--success);
            padding: 1px 6px;
            border-radius: 4px;
            font-weight: 700;
        }
        .sc-cli .ps1 { color: var(--success); margin-right: 6px; font-weight: 700; }
        .sc-cli .ps1g { color: var(--accent); margin-right: 6px; font-weight: 700; }
        .sc-cli .typed { color: var(--text); }
        .sc-cli .caret { color: var(--accent); animation: blink 1s steps(1, start) infinite; font-weight: 700; }
        .sc-cli .caret-main { display: inline; }
        .sc-cli.st-sent .caret-main { display: none; }
        .sc-cli .enter-key {
            display: none;
            margin-left: 8px;
            width: 22px;
            height: 18px;
            border-radius: 4px;
            background: var(--accent);
            color: var(--white);
            align-items: center;
            justify-content: center;
            font-size: 10px;
            box-shadow: 0 0 0 4px color-mix(in srgb, var(--accent) 20%, transparent);
            vertical-align: middle;
        }
        .sc-cli.st-sent .term-body .enter-key,
        .sc-cli.st-rsent .gui-content .enter-key {
            display: inline-flex;
            animation: enter-flash 360ms ease-out;
        }
        @keyframes enter-flash {
            0%   { transform: scale(1.4); box-shadow: 0 0 0 10px color-mix(in srgb, var(--accent) 30%, transparent); }
            100% { transform: scale(1); box-shadow: 0 0 0 4px color-mix(in srgb, var(--accent) 20%, transparent); }
        }

        /* Left window: after Enter the session attaches to prod in place —
           remote banner + remote prompt replace the zsh prompt. */
        .sc-cli .ln-welcome { color: var(--text-dim); }
        .sc-cli .ln-welcome, .sc-cli .ln-remote { display: none; }
        .sc-cli.st-connect .ln-welcome, .sc-cli.st-connect .ln-remote { display: block; }

        .sc-cli .gui-body { flex: 1; display: flex; min-height: 0; }
        .sc-cli .gui-sidebar {
            width: 44px;
            padding: 6px;
            background: #181920;
            border-right: 1px solid rgba(255, 255, 255, 0.04);
            display: flex;
            flex-direction: column;
            gap: 4px;
            align-items: center;
        }
        .sc-cli .gui-tab {
            width: 32px;
            height: 32px;
            border-radius: 7px;
            background: rgba(255, 255, 255, 0.04);
            color: var(--text-sub);
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 13px;
            font-weight: 700;
            display: inline-flex;
            align-items: center;
            justify-content: center;
            position: relative;
            transition: background 0.18s ease, color 0.18s ease, box-shadow 0.2s ease, width 0.3s ease;
        }
        .sc-cli .gui-sep { width: 22px; height: 1px; background: rgba(255, 255, 255, 0.06); margin: 4px 0; }
        .sc-cli .gui-spacer { flex: 1; }

        .sc-cli .tab-add { color: var(--text-dim); }

        /* Focused tab (local at first, prod once it opens) */
        .sc-cli .gui-tab.active {
            background: color-mix(in srgb, var(--accent) 28%, transparent);
            color: var(--accent);
            box-shadow:
                inset 0 0 0 1px color-mix(in srgb, var(--accent) 50%, transparent),
                0 0 14px color-mix(in srgb, var(--accent) 40%, transparent);
        }

        .sc-cli .tab-prod {
            transform: scale(0.4);
            opacity: 0;
            width: 0;
            overflow: hidden;
            border: 1px solid transparent;
        }
        .sc-cli.st-tab .tab-prod {
            transform: scale(1);
            opacity: 1;
            width: 32px;
            animation: tab-in 520ms cubic-bezier(0.22, 1, 0.36, 1) forwards;
        }
        @keyframes tab-in {
            0%   { transform: scale(0.4) rotate(-8deg); opacity: 0; }
            60%  { transform: scale(1.08) rotate(2deg); opacity: 1; }
            100% { transform: scale(1) rotate(0deg); opacity: 1; }
        }

        .sc-cli .gui-content {
            flex: 1;
            padding: 14px;
            background: var(--bg);
            position: relative;
            overflow: hidden;
        }
        .sc-cli .gui-content-inner { height: 100%; display: flex; flex-direction: column; justify-content: flex-start; }

        /* Right window, phase 1: the focused local terminal types the CLI. */
        .sc-cli .gui-local-term {
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 12px;
            line-height: 1.7;
            color: #d4d8e2;
        }
        .sc-cli.st-tab .gui-local-term { display: none; }
        .sc-cli .g-caret { display: inline; }
        .sc-cli.st-rsent .g-caret { display: none; }
        .sc-cli .g-opened { display: none; color: var(--success); }
        .sc-cli.st-rsent .g-opened { display: block; }

        /* Right window, phase 2: the opened prod tab's session. */
        .sc-cli .gui-term-mock {
            display: none;
            font-family: "SF Mono", Menlo, Consolas, monospace;
            font-size: 12px;
            line-height: 1.7;
            color: #d4d8e2;
            opacity: 0;
        }
        .sc-cli.st-tab .gui-term-mock { display: block; animation: gui-fade 480ms ease forwards; }
        @keyframes gui-fade { to { opacity: 1; } }
        .sc-cli .gln.out { color: var(--text-sub); }

        /* ── responsive + reduced motion ───────────────────────────── */

        @media (max-width: 900px) {
            .rssh-scene .stage { max-height: none; }
        }

        @media (max-width: 760px) {
            /* Scenes are designed ≥640px wide (sync's node grid). zoom 0.5
               renders the stage at visual 100% while its internal layout
               stays at 2× — no re-flow, just a scaled-down view. */
            .rssh-scene .stage {
                width: 100%;
                zoom: 0.5;
            }
            .sc-cli .stage { grid-template-columns: 1fr; grid-template-rows: 1fr 1fr; }
        }

        @media (prefers-reduced-motion: reduce) {
            .rssh-scene *, .rssh-scene *::before, .rssh-scene *::after {
                animation: none !important;
                transition: none !important;
            }
            .rssh-scene.reduce .mock-cursor { opacity: 0 !important; }
            .rssh-scene.reduce .burst-host { display: none !important; }
            .rssh-scene.reduce .scanline { display: none !important; }
        }
    `;

    const SPRITE = `
        <svg width="0" height="0" style="position:absolute" aria-hidden="true">
            <defs>
                <symbol id="i-ai" viewBox="0 0 24 24">
                    <path d="M12 3.5c.8 4.4 3.1 6.7 7.5 7.5-4.4.8-6.7 3.1-7.5 7.5-.8-4.4-3.1-6.7-7.5-7.5 4.4-.8 6.7-3.1 7.5-7.5Z"/>
                    <path d="M19 3v3M20.5 4.5h-3"/>
                </symbol>
                <symbol id="i-check" viewBox="0 0 24 24"><path d="m5 12 4 4 10-10"/></symbol>
                <symbol id="i-docker" viewBox="0 0 24 24">
                    <path d="M4 10h3v3H4zm4 0h3v3H8zm4 0h3v3h-3zM8 6h3v3H8zm4 0h3v3h-3zm4 4h3v3h-3z"/>
                    <path d="M3 14h16c-.6 3.8-3.2 6-7.6 6H9c-3.1 0-5.2-2-6-6Zm16-2c1.4 0 2.2.6 2.7 1.5-1 .7-2 .9-3 .7"/>
                </symbol>
                <symbol id="i-k8s" viewBox="0 0 24 24">
                    <path d="m12 2.8 7.2 4.1v8.2L12 19.2l-7.2-4.1V6.9Z"/>
                    <circle cx="12" cy="11" r="2.5"/>
                    <path d="M12 5.5V8m0 6v2.5M6.8 8l2.1 1.2m6.2 3.6 2.1 1.2m0-6-2.1 1.2m-6.2 3.6L6.8 14"/>
                </symbol>
                <symbol id="i-ssh" viewBox="0 0 24 24">
                    <rect x="3" y="4" width="18" height="16" rx="2"/>
                    <path d="m7 9 3 3-3 3M12 15h5"/>
                </symbol>
                <symbol id="i-home" viewBox="0 0 24 24">
                    <path d="m3 11 9-8 9 8"/>
                    <path d="M5.5 9.5V21h13V9.5M9.5 21v-7h5v7"/>
                </symbol>
                <symbol id="i-add" viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></symbol>
                <symbol id="i-terminal" viewBox="0 0 24 24">
                    <rect x="3" y="4" width="18" height="16" rx="2"/>
                    <path d="m7 9 3 3-3 3M13 15h4"/>
                </symbol>
                <symbol id="i-settings" viewBox="0 0 24 24">
                    <circle cx="12" cy="12" r="3"/>
                    <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-2.8 2.8-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6v.2h-4V21a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1L4.2 17l.1-.1a1.7 1.7 0 0 0 .3-1.9A1.7 1.7 0 0 0 3 14H2.8v-4H3a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9L4.2 7 7 4.2l.1.1A1.7 1.7 0 0 0 9 4.6 1.7 1.7 0 0 0 10 3v-.2h4V3a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1L19.8 7l-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.2v4H21a1.7 1.7 0 0 0-1.6 1Z"/>
                </symbol>
            </defs>
        </svg>
    `;

    const CURSOR = `
        <div class="mock-cursor" aria-hidden="true">
            <svg viewBox="0 0 16 18" width="22" height="24">
                <path d="M1 1 L1 14 L4.5 10.5 L7 16 L9 15 L6.5 9.5 L11.5 9.5 Z"
                      fill="white" stroke="black" stroke-width="1" stroke-linejoin="round"/>
            </svg>
            <span class="ripple"></span>
        </div>
    `;

    const MARKUP = {
        ai: `
            <div class="mock-app">
                <div class="app-header">
                    <div class="dots">
                        <span class="dot r"></span><span class="dot y"></span><span class="dot g"></span>
                    </div>
                    <div class="app-title">rssh — prod-web-01</div>
                    <button class="ai-btn" tabindex="-1">
                        <span class="ai-glyph"><svg class="ic" width="13" height="13"><use href="#i-ai"/></svg></span> AI
                    </button>
                </div>

                <div class="app-body">
                    <div class="term-pane">
                        <div class="ln"><span class="prompt">$</span> <span>uptime</span></div>
                        <div class="ln out">up 14 days · load avg 1.42</div>
                        <div class="ln app"><span class="prompt">$</span> <span>df -h /</span></div>
                        <div class="ln out warn app">/dev/sda1   480G  478G    2G   <span class="hot">100%</span>  /</div>
                        <div class="ln"><span class="prompt">$</span> <span class="cur-blink">_</span></div>
                    </div>

                    <aside class="ai-pane">
                        <div class="ai-head">
                            <span class="ai-dot"></span>
                            <span class="ai-name">AI Diagnose</span>
                            <span class="ai-model">claude-opus-4-7</span>
                        </div>

                        <div class="ai-thread">
                            <div class="bubble user">why is the disk full?</div>
                            <div class="bubble asst dots-bubble">
                                <span class="dots-anim"><span></span><span></span><span></span></span>
                            </div>
                            <div class="bubble asst reply-bubble">Let me see what's eating the disk:</div>
                            <div class="bubble asst card-bubble">
                                <div class="tool-card">
                                    <div class="tool-head">
                                        <span class="tool-tag">tool_use</span>
                                        <span class="tool-name">run_command</span>
                                        <span class="tool-status">
                                            <svg class="ic" width="11" height="11"><use href="#i-check"/></svg> approved
                                        </span>
                                    </div>
                                    <div class="tool-args">df -h /</div>
                                    <div class="tool-guard">
                                        <span class="guard-item">shape <svg class="ic" width="9" height="9"><use href="#i-check"/></svg></span>
                                        <span class="guard-item">redact <svg class="ic" width="9" height="9"><use href="#i-check"/></svg></span>
                                    </div>
                                    <div class="tool-actions">
                                        <button class="approve-btn" tabindex="-1">Approve</button>
                                    </div>
                                </div>
                            </div>
                        </div>

                        <div class="ai-input">
                            <span class="caret-bar">›</span>
                            <span class="typed" data-typed></span>
                            <span class="caret-blink">_</span>
                            <span class="enter-key">⏎</span>
                        </div>
                    </aside>
                </div>
            </div>
            ${CURSOR}
        `,

        blocks: `
            <div class="mock-app">
                <div class="app-header">
                    <div class="dots"><span class="dot r"></span><span class="dot y"></span><span class="dot g"></span></div>
                    <div class="app-title">rssh — staging</div>
                    <span class="header-spacer"></span>
                </div>

                <div class="term-pane">
                    <div class="block" style="--bar: var(--success);">
                        <span class="bar" data-target="bar-0" aria-hidden="true"></span>
                        <div class="block-body">
                            <div class="ln cmd-ln"><span class="prompt">$</span> ls /var/log</div>
                            <div class="ln out">auth.log    52K</div>
                            <div class="ln out">nginx/      4.0K</div>
                            <div class="ln out">syslog      18M</div>
                            <div class="ln out">kern.log    9.2M</div>
                            <div class="ln folded-hint">▶ 4 lines hidden</div>
                        </div>
                    </div>
                    <div class="block" style="--bar: var(--accent);">
                        <span class="bar" data-target="bar-1" aria-hidden="true"></span>
                        <div class="block-body">
                            <div class="ln cmd-ln"><span class="prompt">$</span> df -h /</div>
                            <div class="ln out">Filesystem  Size  Used  Avail  Use%</div>
                            <div class="ln out">/dev/sda1   480G  478G    2G   100%   /</div>
                        </div>
                    </div>
                    <div class="block" style="--bar: var(--warning);">
                        <span class="bar" data-target="bar-2" aria-hidden="true"></span>
                        <div class="block-body">
                            <div class="ln cmd-ln"><span class="prompt">$</span> ps aux | head -3</div>
                            <div class="ln out">USER  PID  %CPU  COMMAND</div>
                            <div class="ln out">root    1   0.0  /sbin/init</div>
                            <div class="ln out">rssh   42   1.8  node server.js</div>
                        </div>
                    </div>
                </div>
            </div>

            <div class="ctx-menu" role="menu" aria-hidden="true">
                <div class="ctx-item" data-menu-item="text">
                    <span class="ctx-label">Copy as text</span>
                    <span class="ctx-key">⌘C</span>
                </div>
                <div class="ctx-item" data-menu-item="image">
                    <span class="ctx-label">Copy as image</span>
                    <span class="ctx-key">⇧⌘C</span>
                </div>
                <div class="ctx-sep"></div>
                <div class="ctx-item" data-menu-item="fold">
                    <span class="ctx-label">Fold</span>
                </div>
            </div>

            <div class="burst-host" aria-hidden="true">
                <div class="thumb t1">$ ls /var/log</div>
                <div class="thumb t2">$ df -h /</div>
                <div class="thumb t3">$ ps aux | head</div>
                <div class="thumb t4">$ uptime</div>
            </div>

            ${CURSOR}
        `,

        discovery: `
            <div class="mock-app">
                <div class="app-header">
                    <div class="dots"><span class="dot r"></span><span class="dot y"></span><span class="dot g"></span></div>
                    <div class="app-title">RSSH</div>
                    <span class="header-spacer"></span>
                </div>

                <div class="sources">
                    <div class="source">
                        <svg class="ic" width="13" height="13"><use href="#i-docker"/></svg>
                        <span class="src-text">docker · dev-remote</span>
                    </div>
                    <div class="source">
                        <svg class="ic" width="13" height="13"><use href="#i-k8s"/></svg>
                        <span class="src-text">kubectl · staging/api</span>
                    </div>
                </div>

                <div class="home-list">
                    <div class="hsection">
                        <div class="hlabel">Profiles</div>
                        <div class="hcards">
                            <div class="tcard"><span class="ticon"><svg class="ic" width="15" height="15"><use href="#i-ssh"/></svg></span>
                                <span class="tbody"><span class="tname">bastion</span><span class="tsub">ops@10.0.0.9:22</span></span></div>
                            <div class="tcard"><span class="ticon"><svg class="ic" width="15" height="15"><use href="#i-ssh"/></svg></span>
                                <span class="tbody"><span class="tname">prod-db</span><span class="tsub">dba@db.int:5432</span></span></div>
                        </div>
                    </div>

                    <div class="hsection h-docker">
                        <div class="hlabel">Docker</div>
                        <div class="hcards">
                            <div class="tcard pop d1" data-flash><span class="ticon"><svg class="ic" width="15" height="15"><use href="#i-docker"/></svg></span>
                                <span class="tbody"><span class="tname">api-1</span><span class="tsub">nginx:1.27 · Up 2h</span></span></div>
                            <div class="tcard pop d2"><span class="ticon"><svg class="ic" width="15" height="15"><use href="#i-docker"/></svg></span>
                                <span class="tbody"><span class="tname">worker-7</span><span class="tsub">worker:2.4 · Up 26h</span></span></div>
                        </div>
                    </div>

                    <div class="hsection h-k8s">
                        <div class="hlabel">Kubernetes</div>
                        <div class="hcards">
                            <div class="tcard pop d1" data-pod><span class="ticon k8s"><svg class="ic" width="15" height="15"><use href="#i-k8s"/></svg></span>
                                <span class="tbody"><span class="tname" data-pod-name>api-7f9c6d</span><span class="tsub" data-pod-sub>api · Running · staging</span></span></div>
                            <div class="tcard pop d2"><span class="ticon k8s"><svg class="ic" width="15" height="15"><use href="#i-k8s"/></svg></span>
                                <span class="tbody"><span class="tname">debug-shell</span><span class="tsub">tools · Running</span></span></div>
                        </div>
                    </div>

                    <div class="scanline" aria-hidden="true"></div>
                </div>

                <div class="term-strip">
                    <div class="tln"><span class="ps1">$</span> <span data-typed></span><span class="caret" data-typing>▍</span></div>
                    <div class="tln t-root"><span class="root">#</span> <span class="caret">▍</span></div>
                </div>
            </div>
        `,

        sync: `
            <svg class="lines" viewBox="0 0 1000 600" preserveAspectRatio="none" aria-hidden="true">
                <path class="connect c1" d="M 160 300 H 190 V 150 H 220" fill="none"/>
                <path class="connect c2" d="M 160 300 H 190 V 450 H 220" fill="none"/>
                <path class="connect c3 encrypted" d="M 380 450 H 415 V 300 H 450" fill="none"/>
                <path class="connect c4" d="M 610 300 H 730 V 102 H 850" fill="none"/>
                <path class="connect c5" d="M 610 300 H 730 V 222 H 850" fill="none"/>
                <path class="connect c6" d="M 610 300 H 730 V 378 H 850" fill="none"/>
                <path class="connect c7" d="M 610 300 H 730 V 498 H 850" fill="none"/>
                <text class="enc-label" x="425" y="378">ENCRYPTED</text>
            </svg>

            <div class="node rssh">
                <div class="node-icon rssh-icon">
                    <span class="rssh-glyph">&gt;_</span>
                </div>
                <div class="node-label">RSSH</div>
                <div class="node-sub">your machine</div>
            </div>

            <div class="node keyserver">
                <div class="node-icon">
                    <svg viewBox="0 0 36 30" width="42" height="36" aria-hidden="true">
                        <rect x="1" y="1" width="34" height="28" rx="4" fill="#2a2c36" stroke="currentColor" stroke-width="1.4"/>
                        <path d="M14 17 V14 a4 4 0 0 1 8 0 V17" fill="none" stroke="currentColor" stroke-width="1.6"/>
                        <rect x="11" y="17" width="14" height="8" rx="1.5" fill="currentColor"/>
                    </svg>
                    <span class="key-stay" aria-hidden="true">
                        <svg viewBox="0 0 22 10" width="22" height="10">
                            <circle cx="4" cy="5" r="3" fill="none" stroke="#fde68a" stroke-width="1.6"/>
                            <rect x="7" y="4.2" width="11" height="1.6" fill="#fde68a"/>
                            <rect x="14" y="5.8" width="1.6" height="2" fill="#fde68a"/>
                            <rect x="17" y="5.8" width="1.6" height="2.5" fill="#fde68a"/>
                        </svg>
                    </span>
                </div>
                <div class="node-label">OS KEY STORE</div>
                <div class="node-sub">Keychain · Cred Mgr · Secret Service</div>
            </div>

            <div class="node db">
                <div class="node-icon">
                    <svg viewBox="0 0 30 30" width="36" height="36" aria-hidden="true">
                        <ellipse cx="15" cy="6" rx="12" ry="3.5" fill="#2a2c36" stroke="currentColor" stroke-width="1.4"/>
                        <path d="M3 6 V14 a12 3.5 0 0 0 24 0 V6" fill="#2a2c36" stroke="currentColor" stroke-width="1.4"/>
                        <path d="M3 14 V22 a12 3.5 0 0 0 24 0 V14" fill="#2a2c36" stroke="currentColor" stroke-width="1.4"/>
                    </svg>
                </div>
                <div class="node-label">PROFILE DB</div>
                <div class="node-sub">host · user · port</div>
            </div>

            <div class="node repo">
                <div class="node-icon repo-icon">
                    <svg viewBox="0 0 48 30" width="54" height="34" aria-hidden="true">
                        <path
                            d="M11 26 C 4 26  2 19  8 16
                               C 7 7  17 4  22 11
                               C 26 5  37 7  37 15
                               C 44 15  44 26  37 26 Z"
                            fill="#2a2c36" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"
                        />
                    </svg>
                    <span class="doc-stored" aria-hidden="true">
                        <svg viewBox="0 0 14 16" width="14" height="16">
                            <path d="M2 1 H10 L13 4 V15 H2 Z" fill="#1c1d24" stroke="#7dd3fc" stroke-width="1.2" stroke-linejoin="round"/>
                            <line x1="4" y1="6" x2="11" y2="6" stroke="#7dd3fc" stroke-width="1.2"/>
                            <line x1="4" y1="9" x2="11" y2="9" stroke="#7dd3fc" stroke-width="1.2"/>
                            <line x1="4" y1="12" x2="9" y2="12" stroke="#7dd3fc" stroke-width="1.2"/>
                        </svg>
                    </span>
                    <div class="lock-badge">
                        <svg viewBox="0 0 14 18" width="18" height="22" aria-hidden="true">
                            <path d="M3 8 V6 a4 4 0 0 1 8 0 V8" fill="none" stroke="currentColor" stroke-width="1.6"/>
                            <rect x="1.5" y="8" width="11" height="8.5" rx="1.5" fill="currentColor"/>
                            <circle cx="7" cy="12" r="1" fill="#1c1d24"/>
                        </svg>
                    </div>
                </div>
                <div class="node-label">GITHUB REPO</div>
                <div class="node-sub">encrypted backup</div>
            </div>

            <div class="node platform p-win" style="--pf: #4a8bf7;">
                <div class="pf-tile">W</div>
                <div class="node-label-sm">Windows</div>
            </div>
            <div class="node platform p-mac" style="--pf: #b0b7c4;">
                <div class="pf-tile">M</div>
                <div class="node-label-sm">macOS</div>
            </div>
            <div class="node platform p-lin" style="--pf: #f3a142;">
                <div class="pf-tile">L</div>
                <div class="node-label-sm">Linux</div>
            </div>
            <div class="node platform p-and" style="--pf: #4cb88a;">
                <div class="pf-tile">A</div>
                <div class="node-label-sm">Android</div>
            </div>

            <div class="doc d-encrypt" aria-hidden="true">
                <svg viewBox="0 0 14 16" width="16" height="18">
                    <path d="M2 1 H10 L13 4 V15 H2 Z" fill="#1c1d24" stroke="#7dd3fc" stroke-width="1.2" stroke-linejoin="round"/>
                    <line x1="4" y1="6" x2="11" y2="6" stroke="#7dd3fc" stroke-width="1.2"/>
                    <line x1="4" y1="9" x2="11" y2="9" stroke="#7dd3fc" stroke-width="1.2"/>
                    <line x1="4" y1="12" x2="9" y2="12" stroke="#7dd3fc" stroke-width="1.2"/>
                </svg>
            </div>
            <div class="doc d-out-1" aria-hidden="true">
                <svg viewBox="0 0 14 16" width="14" height="16">
                    <path d="M2 1 H10 L13 4 V15 H2 Z" fill="#1c1d24" stroke="#7dd3fc" stroke-width="1.2" stroke-linejoin="round"/>
                    <line x1="4" y1="6" x2="11" y2="6" stroke="#7dd3fc" stroke-width="1.2"/>
                    <line x1="4" y1="9" x2="11" y2="9" stroke="#7dd3fc" stroke-width="1.2"/>
                </svg>
            </div>
            <div class="doc d-out-2" aria-hidden="true">
                <svg viewBox="0 0 14 16" width="14" height="16">
                    <path d="M2 1 H10 L13 4 V15 H2 Z" fill="#1c1d24" stroke="#7dd3fc" stroke-width="1.2" stroke-linejoin="round"/>
                    <line x1="4" y1="6" x2="11" y2="6" stroke="#7dd3fc" stroke-width="1.2"/>
                    <line x1="4" y1="9" x2="11" y2="9" stroke="#7dd3fc" stroke-width="1.2"/>
                </svg>
            </div>
            <div class="doc d-out-3" aria-hidden="true">
                <svg viewBox="0 0 14 16" width="14" height="16">
                    <path d="M2 1 H10 L13 4 V15 H2 Z" fill="#1c1d24" stroke="#7dd3fc" stroke-width="1.2" stroke-linejoin="round"/>
                    <line x1="4" y1="6" x2="11" y2="6" stroke="#7dd3fc" stroke-width="1.2"/>
                    <line x1="4" y1="9" x2="11" y2="9" stroke="#7dd3fc" stroke-width="1.2"/>
                </svg>
            </div>
            <div class="doc d-out-4" aria-hidden="true">
                <svg viewBox="0 0 14 16" width="14" height="16">
                    <path d="M2 1 H10 L13 4 V15 H2 Z" fill="#1c1d24" stroke="#7dd3fc" stroke-width="1.2" stroke-linejoin="round"/>
                    <line x1="4" y1="6" x2="11" y2="6" stroke="#7dd3fc" stroke-width="1.2"/>
                    <line x1="4" y1="9" x2="11" y2="9" stroke="#7dd3fc" stroke-width="1.2"/>
                </svg>
            </div>
        `,

        cli: `
            <div class="mock-app term-side">
                <div class="app-header">
                    <div class="dots"><span class="dot r"></span><span class="dot y"></span><span class="dot g"></span></div>
                    <div class="app-title">~ — zsh</div>
                    <span class="header-spacer"></span>
                </div>
                <div class="term-body">
                    <div class="ln dim">Last login on ttys004</div>
                    <div class="ln"><span class="ps1">~ ❯</span> <span class="typed" data-typed></span><span class="caret caret-main">_</span><span class="enter-key">⏎</span></div>
                    <div class="ln ln-welcome">Welcome to prod-web-01 · Ubuntu 24.04 LTS</div>
                    <div class="ln ln-remote"><span class="ps1">deploy@prod:~$</span> <span class="caret">_</span></div>
                </div>
            </div>

            <div class="mock-app gui-side">
                <div class="app-header">
                    <div class="dots"><span class="dot r"></span><span class="dot y"></span><span class="dot g"></span></div>
                    <div class="app-title">RSSH</div>
                    <span class="header-spacer"></span>
                </div>
                <div class="gui-body">
                    <nav class="gui-sidebar">
                        <div class="gui-tab tab-home" title="Home"><svg class="ic" width="14" height="14"><use href="#i-home"/></svg></div>
                        <div class="gui-tab tab-add" title="New"><svg class="ic" width="14" height="14"><use href="#i-add"/></svg></div>
                        <div class="gui-sep"></div>
                        <div class="gui-tab tab-local active" title="local"><svg class="ic" width="14" height="14"><use href="#i-terminal"/></svg></div>
                        <div class="gui-tab tab-prod" title="prod">
                            <span class="tab-letter"><svg class="ic" width="14" height="14"><use href="#i-ssh"/></svg></span>
                        </div>
                        <div class="gui-spacer"></div>
                    </nav>
                    <div class="gui-content">
                        <div class="gui-content-inner">
                            <div class="gui-local-term">
                                <div class="gln"><span class="ps1">~ ❯</span> <span class="typed" data-typed2></span><span class="caret g-caret">▍</span><span class="enter-key">⏎</span></div>
                                <div class="gln g-opened">→ opened <span class="hl">ssh:prod</span></div>
                            </div>
                            <div class="gui-term-mock">
                                <div class="gln"><span class="ps1g">prod ❯</span> uptime</div>
                                <div class="gln out">up 14 days · load 1.42</div>
                                <div class="gln"><span class="ps1g">prod ❯</span> <span class="caret">_</span></div>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        `,
    };

    /* ── player ─────────────────────────────────────────────────── */

    const typeBeats = (start, msPerChar, text, apply) => {
        const beats = [];
        for (let i = 1; i <= text.length; i++) {
            beats.push([start + i * msPerChar, () => apply(text.slice(0, i))]);
        }
        return beats;
    };

    const SCENES = {
        ai: {
            loop: 9600,
            beats(fig) {
                const PROMPT = 'why is the disk full?';
                const stage = fig.querySelector('.stage');
                const cursor = fig.querySelector('.mock-cursor');
                const typed = fig.querySelector('[data-typed]');
                const typeEnd = 2500 + PROMPT.length * 55;
                // Aim at measured targets (AI button, Approve button) —
                // never hand-tuned %, the stage aspect differs from the
                // welcome screen's.
                const moveTo = (sel) => () => {
                    const t = stage.querySelector(sel);
                    if (!t) return;
                    const s = stage.getBoundingClientRect();
                    const r = t.getBoundingClientRect();
                    cursor.style.left = (r.left - s.left + r.width / 2).toFixed(1) + 'px';
                    cursor.style.top = (r.top - s.top + r.height / 2).toFixed(1) + 'px';
                };
                return [
                    [300, () => cursor.classList.add('visible')],
                    [500, moveTo('.ai-btn')],
                    [1500, () => cursor.classList.add('clicking')],
                    [1620, () => { cursor.classList.remove('clicking'); fig.classList.add('st-open'); }],
                    [2050, () => fig.classList.add('st-focus')],
                    ...typeBeats(2500, 55, PROMPT, (t) => { typed.textContent = t; }),
                    [typeEnd + 280, () => fig.classList.add('st-sent')],
                    [typeEnd + 950, () => fig.classList.add('st-reply')],
                    [typeEnd + 1650, () => fig.classList.add('st-tool')],
                    // Measure only after the card's entry animation (300ms
                    // delay + 380ms) has fully settled, so the target is final.
                    [typeEnd + 2450, moveTo('.approve-btn')],
                    [typeEnd + 3950, () => { cursor.classList.add('clicking'); fig.classList.add('st-press'); }],
                    [typeEnd + 4350, () => {
                        cursor.classList.remove('clicking');
                        fig.classList.remove('st-press');
                        fig.classList.add('st-approved');
                    }],
                ];
            },
            reset(fig) {
                fig.classList.remove('st-open', 'st-focus', 'st-sent', 'st-reply', 'st-tool', 'st-press', 'st-approved');
                fig.querySelector('[data-typed]').textContent = '';
                const cursor = fig.querySelector('.mock-cursor');
                cursor.classList.remove('visible', 'clicking');
                cursor.style.left = '108%';
                cursor.style.top = '108%';
            },
        },

        blocks: {
            loop: 12100,
            beats(fig) {
                const stage = fig.querySelector('.stage');
                const cursor = fig.querySelector('.mock-cursor');
                const blocks = [...fig.querySelectorAll('.block')];
                const menu = fig.querySelector('.ctx-menu');
                const items = [...fig.querySelectorAll('.ctx-item')];
                const burst = fig.querySelector('.burst-host');

                const moveTo = (sel) => {
                    const target = stage.querySelector(sel);
                    if (!target) return;
                    const s = stage.getBoundingClientRect();
                    const t = target.getBoundingClientRect();
                    cursor.style.left = (t.left - s.left + t.width / 2).toFixed(1) + 'px';
                    cursor.style.top = (t.top - s.top + t.height / 2).toFixed(1) + 'px';
                };
                const offStage = () => {
                    const r = stage.getBoundingClientRect();
                    cursor.style.left = (r.width + 24) + 'px';
                    cursor.style.top = (r.height + 24) + 'px';
                };
                const click = (on) => cursor.classList.toggle('clicking', on);
                const openMenu = (i) => {
                    menu.style.top = i === 0 ? '22%' : '38%';
                    menu.classList.add('open');
                };
                const closeMenu = () => {
                    menu.classList.remove('open');
                    items.forEach((it) => it.classList.remove('hovered'));
                };
                const select = (i) => {
                    fig.classList.add('st-focus');
                    blocks.forEach((b, j) => {
                        b.classList.toggle('selected', j === i);
                        b.classList.toggle('dim', j !== i);
                    });
                };
                const deselect = () => {
                    fig.classList.remove('st-focus');
                    blocks.forEach((b) => b.classList.remove('selected', 'dim'));
                };

                return [
                    [0, offStage],
                    [400, () => blocks[0].classList.add('lit')],
                    [700, () => blocks[1].classList.add('lit')],
                    [1000, () => blocks[2].classList.add('lit')],

                    // Beat 1: click middle bar → copy as image → burst
                    [1300, () => cursor.classList.add('visible')],
                    [1500, () => moveTo('[data-target="bar-1"]')],
                    [2500, () => click(true)],
                    [2620, () => { click(false); select(1); }],
                    [2900, () => openMenu(1)],
                    [3600, () => moveTo('[data-menu-item="image"]')],
                    [4100, () => fig.querySelector('[data-menu-item="image"]').classList.add('hovered')],
                    [4400, () => click(true)],
                    [4520, () => {
                        click(false);
                        closeMenu();
                        deselect();
                        burst.style.left = cursor.style.left;
                        burst.style.top = cursor.style.top;
                        burst.classList.add('on');
                    }],
                    [5400, () => burst.classList.remove('on')],

                    // Beat 2: click top bar → fold
                    [5500, () => moveTo('[data-target="bar-0"]')],
                    [6400, () => click(true)],
                    [6520, () => { click(false); select(0); openMenu(0); }],
                    [7200, () => moveTo('[data-menu-item="fold"]')],
                    [7700, () => fig.querySelector('[data-menu-item="fold"]').classList.add('hovered')],
                    [8000, () => click(true)],
                    [8120, () => {
                        click(false);
                        closeMenu();
                        deselect();
                        blocks[0].classList.add('folded');
                    }],
                ];
            },
            reset(fig) {
                fig.classList.remove('st-focus');
                fig.querySelectorAll('.block').forEach((b) => b.classList.remove('lit', 'selected', 'dim', 'folded'));
                const menu = fig.querySelector('.ctx-menu');
                menu.classList.remove('open');
                fig.querySelectorAll('.ctx-item').forEach((it) => it.classList.remove('hovered'));
                fig.querySelector('.burst-host').classList.remove('on');
                const cursor = fig.querySelector('.mock-cursor');
                cursor.classList.remove('visible', 'clicking');
            },
        },

        discovery: {
            loop: 8300,
            beats(fig) {
                const CMD = 'docker exec -it api-1 sh';
                const typed = fig.querySelector('[data-typed]');
                const typingCaret = fig.querySelector('[data-typing]');
                const podName = fig.querySelector('[data-pod-name]');
                const podSub = fig.querySelector('[data-pod-sub]');
                const podCard = fig.querySelector('[data-pod]');
                const typeEnd = 3800 + CMD.length * 45;
                return [
                    [400, () => fig.classList.add('st-sources')],
                    [1000, () => fig.classList.add('st-scan')],
                    [1500, () => fig.classList.add('st-docker')],
                    [2100, () => fig.classList.add('st-k8s')],
                    [2900, () => {
                        podName.textContent = 'api-8h2d1b';
                        podSub.textContent = 'api · Running · 4m';
                        podCard.classList.remove('pop');
                        void podCard.offsetWidth;
                        podCard.classList.add('pop');
                    }],
                    [3600, () => {
                        fig.classList.add('st-open');
                        const flash = fig.querySelector('[data-flash]');
                        flash.classList.remove('flash');
                        void flash.offsetWidth;
                        flash.classList.add('flash');
                    }],
                    ...typeBeats(3800, 45, CMD, (t) => { typed.textContent = t; }),
                    [typeEnd, () => { typingCaret.style.display = 'none'; fig.classList.add('st-typed'); }],
                ];
            },
            reset(fig) {
                fig.classList.remove('st-sources', 'st-scan', 'st-docker', 'st-k8s', 'st-open', 'st-typed');
                fig.querySelector('[data-typed]').textContent = '';
                const caret = fig.querySelector('[data-typing]');
                if (caret) caret.style.display = '';
                fig.querySelector('[data-pod-name]').textContent = 'api-7f9c6d';
                fig.querySelector('[data-pod-sub]').textContent = 'api · Running · staging';
                const flash = fig.querySelector('[data-flash]');
                if (flash) flash.classList.remove('flash');
            },
        },

        sync: {
            loop: 8700,
            beats(fig) {
                return [
                    [400, () => fig.classList.add('s-nodes')],
                    [1500, () => fig.classList.add('s-wired')],
                    [2100, () => fig.classList.add('s-encrypting')],
                    [3300, () => fig.classList.add('s-stored')],
                    [3700, () => fig.classList.add('s-fanout')],
                    [5100, () => fig.classList.add('s-lit')],
                ];
            },
            reset(fig) {
                fig.classList.remove('s-nodes', 's-wired', 's-encrypting', 's-stored', 's-fanout', 's-lit');
            },
        },

        cli: {
            loop: 8900,
            beats(fig) {
                const CMD = 'rssh profile open prod';
                const typed = fig.querySelector('[data-typed]');
                const typed2 = fig.querySelector('[data-typed2]');
                // Act 1 (left, external zsh): type ⏎ → session attaches to prod.
                const typeEnd = 600 + CMD.length * 70;
                // Act 2 (right, rssh GUI): starts once the left has settled —
                // its focused local terminal runs the same CLI, which opens
                // the prod tab inside the GUI. No cross-window linkage.
                const rStart = typeEnd + 1200;
                const rTypeEnd = rStart + CMD.length * 55;
                return [
                    ...typeBeats(600, 70, CMD, (t) => { typed.textContent = t; }),
                    [typeEnd + 240, () => fig.classList.add('st-sent')],
                    [typeEnd + 750, () => fig.classList.add('st-connect')],
                    ...typeBeats(rStart, 55, CMD, (t) => { typed2.textContent = t; }),
                    [rTypeEnd + 200, () => fig.classList.add('st-rsent')],
                    [rTypeEnd + 750, () => {
                        fig.classList.add('st-tab');
                        fig.querySelector('.tab-local').classList.remove('active');
                        fig.querySelector('.tab-prod').classList.add('active');
                    }],
                ];
            },
            reset(fig) {
                fig.classList.remove('st-sent', 'st-connect', 'st-rsent', 'st-tab');
                fig.querySelector('[data-typed]').textContent = '';
                fig.querySelector('[data-typed2]').textContent = '';
                fig.querySelector('.tab-local').classList.add('active');
                fig.querySelector('.tab-prod').classList.remove('active');
            },
        },
    };

    function stopTimers(fig) {
        (fig._sceneTimers || []).forEach(clearTimeout);
        fig._sceneTimers = [];
    }

    function start(fig, reduceMotion) {
        stopTimers(fig);
        const def = SCENES[fig.dataset.scene];
        if (!def) return;
        def.reset(fig);
        if (reduceMotion) {
            fig.classList.add('reduce');
            def.beats(fig).forEach(([, fn]) => fn());
            return;
        }
        fig._sceneTimers = def.beats(fig).map(([ms, fn]) => setTimeout(fn, ms));
        fig._sceneTimers.push(setTimeout(() => start(fig, reduceMotion), def.loop));
    }

    function boot() {
        const style = document.createElement('style');
        style.textContent = CSS;
        document.head.appendChild(style);
        document.body.insertAdjacentHTML('afterbegin', SPRITE.trim());

        const reduceMotion =
            window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;

        const observer = new IntersectionObserver((entries) => {
            entries.forEach((entry) => {
                const fig = entry.target;
                const def = SCENES[fig.dataset.scene];
                if (!def) return;
                if (entry.isIntersecting) {
                    start(fig, reduceMotion);
                } else {
                    stopTimers(fig);
                    def.reset(fig);
                }
            });
        }, { threshold: 0.3 });

        document.querySelectorAll('[data-scene]').forEach((fig) => {
            const name = fig.dataset.scene;
            if (!MARKUP[name]) return;
            fig.classList.add('rssh-scene', 'sc-' + name);
            fig.innerHTML = '<div class="stage">' + MARKUP[name].trim() + '</div>';
            observer.observe(fig);
        });
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', boot);
    } else {
        boot();
    }
})();
