<script lang="ts">
  let { color }: { color: string } = $props();
  const uid = $props.id();
  const wavePath = "M100 0 C20 66 20 134 100 200 C180 266 180 334 100 400 C20 466 20 534 100 600 C180 666 180 734 100 800";
</script>

<div class="environment-wave" style:--group-wave-color={color} aria-hidden="true">
  {#each [0, 1, 2] as layer}
    <svg viewBox="0 0 200 800" preserveAspectRatio="none" focusable="false" style:width={`${100 - layer * 2}%`} style:transform={`translateY(${-layer * 9.75}%)`} style:--layer-color={`color-mix(in srgb, var(--group-wave-color), white ${layer * 4}%)`}>
      <defs>
        <linearGradient id={`${uid}-${layer}`} x1="0" y1="0" x2="1" y2="0">
          <stop offset="0%" stop-color="var(--layer-color)" stop-opacity={0.08 + layer * 0.012} />
          <stop offset="60%" stop-color="var(--layer-color)" stop-opacity={0.16 + layer * 0.015} />
          <stop offset="100%" stop-color="var(--layer-color)" stop-opacity={0.22 + layer * 0.012} />
        </linearGradient>
      </defs>
      <path d={`${wavePath} H200 V0 Z`} fill={`url(#${uid}-${layer})`} />
      <path d={wavePath} fill="none" stroke="var(--layer-color)" stroke-width="0.7" stroke-opacity="0.12" />
    </svg>
  {/each}
</div>

<style>
  .environment-wave {
    position: absolute;
    inset: 0 0 0 auto;
    width: min(23%, 230px);
    overflow: hidden;
    pointer-events: none;
    user-select: none;
    z-index: 0;
  }
  svg { position: absolute; top: 0; right: 0; height: 290%; }
</style>
