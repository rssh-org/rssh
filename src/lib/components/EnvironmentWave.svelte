<script lang="ts">
  let { color, name, foreground = "var(--text)", compact = false, showName = true }: {
    color: string;
    name: string;
    foreground?: string;
    compact?: boolean;
    showName?: boolean;
  } = $props();

  const uid = $props.id();
</script>

<div class="environment-wave" class:compact style:--group-wave-color={color} style:color={foreground}>
  <svg viewBox="0 0 1440 160" preserveAspectRatio="none" aria-hidden="true" focusable="false">
    <defs>
      <linearGradient id={`${uid}-fill`} x1="0" y1="0" x2="1" y2="0.8">
        <stop offset="0%" stop-color="var(--group-wave-color)" stop-opacity="0.18" />
        <stop offset="52%" stop-color="var(--group-wave-color)" stop-opacity="0.38" />
        <stop offset="100%" stop-color="var(--group-wave-color)" stop-opacity="0.72" />
      </linearGradient>
      <linearGradient id={`${uid}-back`} x1="0" y1="0" x2="1" y2="1">
        <stop offset="0%" stop-color="var(--group-wave-color)" stop-opacity="0.10" />
        <stop offset="100%" stop-color="var(--group-wave-color)" stop-opacity="0.24" />
      </linearGradient>
    </defs>
    <path d="M0 45 C180 -8 245 14 410 64 S650 18 810 58 S1130 73 1260 43 S1370 26 1440 28 V160 H0 Z" fill={`url(#${uid}-back)`} />
    <path d="M0 67 C210 2 300 15 515 67 S830 112 1040 87 S1270 30 1440 60 V160 H0 Z" fill={`url(#${uid}-fill)`} />
  </svg>
  {#if showName}
    <div class="group-name" title={name}><span aria-hidden="true">—</span> {name}</div>
  {/if}
</div>

<style>
  .environment-wave {
    position: relative;
    flex: 0 1 auto;
    width: 100%;
    height: min(15%, 150px);
    min-height: 0;
    overflow: hidden;
    pointer-events: none;
    user-select: none;
  }
  svg { display: block; width: 100%; height: 100%; }
  .group-name {
    position: absolute;
    right: 24px;
    bottom: 16px;
    max-width: calc(100% - 48px);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--term-font-family, monospace);
    font-size: 12px;
    letter-spacing: 0.08em;
    color: inherit;
    opacity: 0.72;
  }
  .group-name span { margin-right: 8px; opacity: 0.5; }
  .compact { height: 56px; }
  .compact .group-name { right: 12px; bottom: 10px; font-size: 11px; }
  @media (max-width: 600px) {
    .environment-wave { max-height: 48px; }
    .group-name { right: 16px; bottom: 10px; font-size: 11px; }
  }
</style>
