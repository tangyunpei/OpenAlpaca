<script lang="ts">
  import { cn } from "./cn";
  import type { HTMLTextareaAttributes } from "svelte/elements";

  interface Props extends HTMLTextareaAttributes {
    class?: string;
  }

  let { class: className, ...rest }: Props = $props();
</script>

<textarea
  class={cn(
    "flex min-h-[60px] w-full rounded-md border border-input bg-card px-3 py-2 text-sm text-foreground shadow-sm transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 resize-y font-[inherit]",
    className,
  )}
  {...rest}
></textarea>
