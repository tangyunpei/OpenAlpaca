<script lang="ts">
  import { cn } from "./cn";
  import { badgeVariants, type BadgeVariants } from "./badge";
  import type { Snippet } from "svelte";

  interface Props {
    variant?: BadgeVariants["variant"];
    class?: string;
    children: Snippet;
  }

  let { variant, class: className, children }: Props = $props();
</script>

<span class={cn(badgeVariants({ variant }), className)}>
  {@render children()}
</span>
