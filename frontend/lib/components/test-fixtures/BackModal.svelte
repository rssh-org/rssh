<script lang="ts">
  import type { Component } from "svelte";
  import Modal from "../Modal.svelte";

  let { dropdown: Dropdown, onClose }: { dropdown: Component<any>; onClose: () => void } = $props();
  let open = $state(true);
</script>

{#if open}
  <Modal onClose={() => { open = false; onClose(); }}>
    <Dropdown
      value="a"
      options={[{ value: "a", label: "A" }, { value: "b", label: "B" }]}
      ariaLabel="Choice"
    />
  </Modal>
{/if}
