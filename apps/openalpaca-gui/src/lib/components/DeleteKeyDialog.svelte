<script lang="ts">
  import { removeKey } from "$lib/api/settings";

  interface Props {
    keyId: string;
    provider: string;
    onClose: () => void;
    onDeleted: () => void;
  }

  let { keyId, provider, onClose, onDeleted }: Props = $props();

  let deleting = $state(false);
  let error = $state<string | null>(null);

  async function handleConfirm() {
    deleting = true;
    error = null;
    try {
      await removeKey(provider, keyId);
      onDeleted();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      deleting = false;
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="fixed inset-0 bg-black/60 flex justify-center items-center z-[1000]" onclick={onClose} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div
    class="bg-surface p-6 rounded-xl w-[90%] max-w-[420px] shadow-[0_4px_20px_rgba(0,0,0,0.3)] border border-border"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
  >
    <h3 class="m-0 mb-3 text-[1.1rem] text-foreground">Remove API Key</h3>
    <p class="text-sm text-muted-foreground m-0 mb-4 leading-relaxed">
      Remove key <strong class="text-foreground">'{keyId}'</strong> from <strong class="text-foreground">{provider}</strong>?
    </p>

    {#if error}
      <div class="bg-danger/20 border border-danger text-danger px-3.5 py-2.5 rounded-lg mb-3 text-[0.85rem]">{error}</div>
    {/if}

    <div class="flex justify-end gap-2.5">
      <button
        class="px-4 py-2 text-sm bg-danger text-white border-none rounded-lg cursor-pointer hover:opacity-90 transition-opacity disabled:opacity-50 disabled:cursor-not-allowed"
        onclick={handleConfirm}
        disabled={deleting}
      >
        {deleting ? "Removing..." : "Remove"}
      </button>
      <button
        class="px-4 py-2 text-sm bg-white/5 text-muted-foreground border border-input rounded-lg cursor-pointer hover:bg-white/10 transition-colors"
        onclick={onClose}
      >Cancel</button>
    </div>
  </div>
</div>
