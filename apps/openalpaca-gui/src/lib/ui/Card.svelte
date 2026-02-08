<script lang="ts">
  import { cn } from "./cn";
  import type { Snippet } from "svelte";

  interface Props {
    class?: string;
    children: Snippet;
    header?: Snippet;
  }

  let { class: className, children, header }: Props = $props();
</script>

<div class={cn("oa-panel", className)}>
  {#if header}
    <div class="oa-panel-header">
      {@render header()}
    </div>
  {/if}
  {@render children()}
</div>
