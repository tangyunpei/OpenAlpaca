<script lang="ts">
  import { cn } from "./cn";
  import type { Snippet } from "svelte";

  interface Props {
    open?: boolean;
    onClose: () => void;
    class?: string;
    children: Snippet;
  }

  let { open = true, onClose, class: className, children }: Props = $props();

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) onClose();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") onClose();
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
    onclick={handleBackdropClick}
    onkeydown={handleKeydown}
  >
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_interactive_supports_focus -->
    <div
      class={cn(
        "relative w-[90%] max-w-[560px] max-h-[80vh] overflow-y-auto rounded-xl border border-border bg-card p-6 shadow-2xl",
        className,
      )}
      onclick={(e) => e.stopPropagation()}
      role="dialog"
    >
      {@render children()}
    </div>
  </div>
{/if}
