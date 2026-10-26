<!--
  Scene 3 — Dynamic Discovery (Docker/K8s), modelled after the real Home:

      source (persisted)          →  docker · dev-remote   /  kubectl · staging/api
      targets (live, not stored)  →  Home sections: Profiles / Docker / Kubernetes
      open                        →  docker exec -it api-1 sh  (a plain PTY tab)

  Beats: source pills drop in → a scan sweep crosses Home → Docker and
  Kubernetes sections grow their cards → one pod is rolled (name changes,
  card re-pops) → the api-1 card flashes and an exec terminal strip slides
  up, typing the docker exec command. Static SSH cards never move: only
  the source is config, targets just appear.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "../../i18n/index.svelte.ts";
  import NextButton from "./NextButton.svelte";
  import AppIcon from "../AppIcon.svelte";

  let { onNext }: { onNext: () => void } = $props();

  let sourcesShown = $state(false);
  let scanning = $state(false);
  let dockerShown = $state(false);
  let k8sShown = $state(false);
  let rolled = $state(false);
  let opened = $state(false);
  let captionShown = $state(false);
  let ready = $state(false);

  const CMD = "docker exec -it api-1 sh";
  let typed = $state("");

  // The rolled pod: old ReplicaSet pod fades out, the new one takes its slot.
  let podName = $state("api-7f9c6d");
  let podSub = $state("api · Running · staging");

  let timers: number[] = [];
  function at(ms: number, fn: () => void) { timers.push(window.setTimeout(fn, ms)); }

  onMount(() => {
    const reduced =
      typeof window !== "undefined" &&
      window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

    if (reduced) {
      sourcesShown = true; scanning = true; dockerShown = true; k8sShown = true;
      rolled = true; opened = true; typed = CMD;
      captionShown = true; ready = true;
      return;
    }

    at(400,  () => { sourcesShown = true; });
    at(1000, () => { scanning = true; });
    at(1500, () => { dockerShown = true; });
    at(2100, () => { k8sShown = true; });
    at(2900, () => {
      rolled = true;
      podName = "api-8h2d1b";
      podSub = "api · Running · 4m";
    });
    at(3600, () => { opened = true; });
    for (let i = 1; i <= CMD.length; i++) {
      at(3800 + i * 45, () => { typed = CMD.slice(0, i); });
    }
    const typeEnd = 3800 + CMD.length * 45;
    at(typeEnd + 300, () => { captionShown = true; });
    at(typeEnd + 1000, () => { ready = true; });

    return () => { timers.forEach(window.clearTimeout); };
  });
</script>

<section class="scene">
  <div class="chip">
    <span class="chip-dot"></span>
    {t("welcome.scene.discovery.chip")}
  </div>

  <div class="stage">
    <div class="mock-app">
      <div class="app-header">
        <div class="dots"><span class="dot r"></span><span class="dot y"></span><span class="dot g"></span></div>
        <div class="app-title">RSSH</div>
        <span class="header-spacer"></span>
      </div>

      <!-- Persisted sources. This row is ALL the config dynamic discovery has. -->
      <div class="sources" class:shown={sourcesShown} class:scanning>
        <div class="source">
          <AppIcon name="docker" size={13} />
          <span class="src-text">docker · dev-remote</span>
        </div>
        <div class="source">
          <AppIcon name="kubernetes" size={13} />
          <span class="src-text">kubectl · staging/api</span>
        </div>
      </div>

      <!-- Live Home: static profiles stay put; discovered sections appear. -->
      <div class="home-list">
        <div class="hsection">
          <div class="hlabel">{t("settings.section.profiles")}</div>
          <div class="hcards">
            <div class="tcard"><span class="ticon"><AppIcon name="ssh" size={15} /></span>
              <span class="tbody"><span class="tname">bastion</span><span class="tsub">ops@10.0.0.9:22</span></span></div>
            <div class="tcard"><span class="ticon"><AppIcon name="ssh" size={15} /></span>
              <span class="tbody"><span class="tname">prod-db</span><span class="tsub">dba@db.int:5432</span></span></div>
          </div>
        </div>

        {#if dockerShown}
          <div class="hsection grow">
            <div class="hlabel">{t("home.type.docker")}</div>
            <div class="hcards">
              <div class="tcard pop d1" class:flash={opened}><span class="ticon docker"><AppIcon name="docker" size={15} /></span>
                <span class="tbody"><span class="tname">api-1</span><span class="tsub">nginx:1.27 · Up 2h</span></span></div>
              <div class="tcard pop d2"><span class="ticon docker"><AppIcon name="docker" size={15} /></span>
                <span class="tbody"><span class="tname">worker-7</span><span class="tsub">worker:2.4 · Up 26h</span></span></div>
            </div>
          </div>
        {/if}

        {#if k8sShown}
          <div class="hsection grow">
            <div class="hlabel">{t("home.type.kubernetes")}</div>
            <div class="hcards">
              {#key podName}
                <div class="tcard pop d1"><span class="ticon k8s"><AppIcon name="kubernetes" size={15} /></span>
                  <span class="tbody"><span class="tname">{podName}</span><span class="tsub">{podSub}</span></span></div>
              {/key}
              <div class="tcard pop d2"><span class="ticon k8s"><AppIcon name="kubernetes" size={15} /></span>
                <span class="tbody"><span class="tname">debug-shell</span><span class="tsub">tools · Running</span></span></div>
            </div>
          </div>
        {/if}

        <div class="scanline" class:on={scanning} aria-hidden="true"></div>
      </div>

      <!-- The opened container: a plain local exec, no agent anywhere. -->
      {#if opened}
        <div class="term-strip">
          <div class="tln"><span class="ps1">$</span> {typed}{#if typed.length < CMD.length}<span class="caret">▍</span>{/if}</div>
          {#if typed === CMD}
            <div class="tln"><span class="root">#</span> <span class="caret">▍</span></div>
          {/if}
        </div>
      {/if}
    </div>
  </div>

  <div class="caption" class:show={captionShown}>
    <span class="kw">{t("welcome.scene.discovery.caption_kw1")}</span>
    {t("welcome.scene.discovery.caption_join")}
    <span class="kw">{t("welcome.scene.discovery.caption_kw2")}</span>
    {t("welcome.scene.discovery.caption_join")}
    <span class="kw">{t("welcome.scene.discovery.caption_kw3")}</span>
  </div>

  <NextButton {ready} onClick={onNext} />
</section>

<style>
  .scene {
    position: relative;
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: clamp(18px, 3vh, 32px);
    padding: clamp(20px, 4vh, 48px) clamp(16px, 3vw, 32px);
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 6px 14px;
    border: 1px solid color-mix(in srgb, var(--success) 55%, transparent);
    border-radius: 999px;
    background: color-mix(in srgb, var(--success) 10%, transparent);
    color: var(--success);
    font-family: "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 1.4px;
    text-transform: uppercase;
    opacity: 0;
    animation: chip-in 500ms 200ms forwards;
  }
  @keyframes chip-in { to { opacity: 1; } }
  .chip-dot {
    width: 6px; height: 6px; border-radius: 50%;
    background: var(--success);
    box-shadow: 0 0 8px var(--success);
    animation: chip-pulse 1.6s ease-in-out infinite;
  }
  @keyframes chip-pulse { 50% { opacity: 0.45; } }

  .stage {
    position: relative;
    width: min(86vw, 980px);
    aspect-ratio: 16 / 10;
    max-height: 66vh;
    opacity: 0;
    transform: translateY(14px);
    animation: stage-in 600ms 100ms cubic-bezier(0.22, 1, 0.36, 1) forwards;
  }
  @keyframes stage-in { to { opacity: 1; transform: translateY(0); } }

  .mock-app {
    height: 100%;
    background: #1c1d24;
    border-radius: 14px;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    box-shadow:
      0 30px 80px rgba(0, 0, 0, 0.6),
      0 0 0 1px rgba(255, 255, 255, 0.05);
  }
  .app-header {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    padding: 10px 14px;
    background: linear-gradient(180deg, #2a2c36 0%, #232530 100%);
    border-bottom: 1px solid rgba(0, 0, 0, 0.4);
    flex-shrink: 0;
  }
  .dots { display: flex; gap: 7px; justify-self: start; }
  .dot { width: 12px; height: 12px; border-radius: 50%; }
  .dot.r { background: #ff5f57; } .dot.y { background: #febc2e; } .dot.g { background: #28c840; }
  .app-title { font-family: "SF Mono", Menlo, Consolas, monospace; font-size: 11px; color: rgba(255,255,255,0.55); letter-spacing: 0.6px; }
  .header-spacer { justify-self: end; }

  /* ── Source pills — the only persisted config ─────────────────────── */
  .sources {
    display: flex;
    gap: 10px;
    padding: 10px 16px 0;
    opacity: 0;
    transform: translateY(-6px);
    transition: opacity 400ms ease, transform 400ms cubic-bezier(0.22, 1, 0.36, 1);
    flex-shrink: 0;
  }
  .sources.shown { opacity: 1; transform: translateY(0); }
  .source {
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
  .source :global(svg) { color: var(--accent); }
  .source:nth-child(2) :global(svg) { color: var(--purple); }
  .sources.scanning .source {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    box-shadow: 0 0 12px color-mix(in srgb, var(--accent) 25%, transparent);
  }

  /* ── Home list ─────────────────────────────────────────────────────── */
  .home-list {
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
  .hsection.grow { animation: section-in 420ms ease both; }
  @keyframes section-in {
    from { opacity: 0; transform: translateY(6px); }
    to   { opacity: 1; transform: translateY(0); }
  }
  .hlabel {
    font-family: "SF Mono", Menlo, Consolas, monospace;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 1.2px;
    text-transform: uppercase;
    color: var(--text-dim);
    margin-bottom: 5px;
  }
  .hcards {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
  }

  .tcard {
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
  .tcard.pop { animation: card-pop 460ms cubic-bezier(0.22, 1, 0.36, 1) both; }
  .tcard.pop.d2 { animation-delay: 120ms; }
  @keyframes card-pop {
    from { opacity: 0; transform: scale(0.85) translateY(8px); }
    to   { opacity: 1; transform: scale(1) translateY(0); }
  }
  .tcard.flash {
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

  .ticon {
    width: 28px; height: 28px;
    border-radius: 8px;
    background: var(--accent-soft);
    color: var(--accent);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .ticon.docker { background: var(--accent-soft); color: var(--accent); }
  .ticon.k8s { background: color-mix(in srgb, var(--purple) 15%, transparent); color: var(--purple); }
  .tbody { display: flex; flex-direction: column; min-width: 0; }
  .tname {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tsub {
    font-family: "SF Mono", Menlo, Consolas, monospace;
    font-size: 10.5px;
    color: var(--text-sub);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Scan sweep — one pass down the list while the contexts are queried. */
  .scanline {
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
  .scanline.on { animation: scan 1000ms cubic-bezier(0.4, 0, 0.6, 1) forwards; }
  @keyframes scan {
    0%   { top: 0; opacity: 0.9; }
    100% { top: 100%; opacity: 0; }
  }

  /* ── Exec terminal strip ──────────────────────────────────────────── */
  .term-strip {
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
    animation: term-in 420ms cubic-bezier(0.22, 1, 0.36, 1) both;
  }
  @keyframes term-in {
    from { opacity: 0; transform: translateY(10px); }
    to   { opacity: 1; transform: translateY(0); }
  }
  .tln { white-space: pre; }
  .ps1 { color: var(--success); font-weight: 700; margin-right: 6px; }
  .root { color: var(--accent); font-weight: 700; margin-right: 6px; }
  .caret { color: var(--accent); animation: blink 1s steps(1, start) infinite; font-weight: 700; }
  @keyframes blink { 50% { opacity: 0; } }

  .caption {
    font-family: "SF Mono", Menlo, Consolas, monospace;
    font-size: clamp(12px, 1.3vw, 14px);
    color: var(--text-sub);
    letter-spacing: 0.6px;
    text-align: center;
    opacity: 0;
    transform: translateY(6px);
    transition: opacity 400ms ease, transform 400ms cubic-bezier(0.22, 1, 0.36, 1);
  }
  .caption.show { opacity: 1; transform: translateY(0); }
  .caption .kw { color: var(--success); font-weight: 700; }

  @media (max-width: 560px) {
    .sources { flex-wrap: wrap; }
    .tsub { display: none; }
  }

  @media (prefers-reduced-motion: reduce) {
    .chip, .stage, .sources, .source, .hsection, .tcard, .scanline,
    .term-strip, .caption, .caret, .chip-dot {
      animation: none !important;
      transition: none !important;
      opacity: 1 !important;
      transform: none !important;
    }
    .scanline { display: none; }
  }
</style>
