<script module lang="ts">
  export const lifecycle = { mounted: 0, destroyed: 0 };
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import Modal from "../Modal.svelte";

  let modalOpen = $state(false);

  onMount(() => {
    lifecycle.mounted += 1;
    return () => { lifecycle.destroyed += 1; };
  });
</script>

<div data-testid="terminal-workspace">
  <textarea aria-label="Terminal input"></textarea>
  <button onclick={() => { modalOpen = true; }}>Open workspace dialog</button>
  {#if modalOpen}
    <Modal onClose={() => { modalOpen = false; }}>Workspace dialog</Modal>
  {/if}
</div>
