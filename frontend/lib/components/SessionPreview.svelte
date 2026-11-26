<script lang="ts">
  import * as app from "../stores/app.svelte.ts";
  import * as theme from "../themes/store.svelte.ts";

  let { tabId, active }: { tabId: string; active: boolean } = $props();
  let lines = $state<string[]>([]);

  $effect(() => {
    if (!active) return;
    const id = tabId;
    const pull = () => { lines = app.readTerminalViewportText(id, 5) ?? []; };
    pull();
    const timer = setInterval(pull, 300);
    return () => clearInterval(timer);
  });
</script>

<pre class="session-preview" style:font-size="{theme.termFontSize()}px">{lines.join("\n")}</pre>

<style>
  .session-preview {
    width: 100%;
    height: 6.25em;
    margin: 0;
    overflow: hidden;
    font-family: var(--term-font);
    font-weight: normal;
    line-height: 1.25;
    white-space: pre;
    background: var(--term-bg);
    color: var(--term-fg);
    border-radius: var(--radius-sm);
  }
</style>
