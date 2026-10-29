<script lang="ts">
  import type { Snippet } from "svelte";
  import { setBackScope } from "../stores/back.svelte.ts";

  let { active, children }: { active: boolean; children: Snippet } = $props();
  setBackScope(() => active);
</script>

{@render children()}
