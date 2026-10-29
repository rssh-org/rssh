<script lang="ts">
  import type { Component } from "svelte";
  import SidePanel from "../SidePanel.svelte";
  import BackModal from "./BackModal.svelte";
  import { canHandleBack } from "../../stores/back.svelte.ts";

  let { hidden, visible, dropdown, onClose, onEnabledChange }: {
    hidden: boolean;
    visible: boolean;
    dropdown: Component<any>;
    onClose: () => void;
    onEnabledChange: (enabled: boolean) => void;
  } = $props();

  $effect(() => { onEnabledChange(canHandleBack()); });
</script>

<SidePanel side="left" width={300} {hidden} panes={[{ id: "saved", visible }]}
  onResizeStart={() => {}} onResetWidth={() => {}}>
  {#snippet pane()}
    <BackModal {dropdown} {onClose} />
  {/snippet}
</SidePanel>
