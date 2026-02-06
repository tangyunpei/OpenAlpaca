<script lang="ts">
  import { removeAgent } from "$lib/stores/agents";

  interface Props {
    agentId: string;
    agentName: string;
    onClose: () => void;
    onDeleted: () => void;
  }

  let { agentId, agentName, onClose, onDeleted }: Props = $props();

  let deleting = $state(false);
  let error = $state<string | null>(null);

  async function handleConfirm() {
    deleting = true;
    error = null;
    try {
      await removeAgent(agentId);
      onDeleted();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      deleting = false;
    }
  }
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div class="modal" onclick={(e) => e.stopPropagation()} role="dialog">
    <h3>Archive Agent</h3>
    <p>
      Archive agent <strong>'{agentName}'</strong>?
      The config file will be moved to <code>.archived/</code>.
    </p>

    {#if error}
      <div class="error-banner">{error}</div>
    {/if}

    <div class="modal-actions">
      <button class="danger" onclick={handleConfirm} disabled={deleting}>
        {deleting ? "Archiving..." : "Archive"}
      </button>
      <button class="secondary" onclick={onClose}>Cancel</button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    top: 0; left: 0; width: 100%; height: 100%;
    background: rgba(0, 0, 0, 0.6);
    display: flex; justify-content: center; align-items: center;
    z-index: 1000;
  }
  .modal {
    background: var(--surface);
    padding: 24px; border-radius: 12px;
    width: 90%; max-width: 420px;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }
  .modal h3 { margin: 0 0 12px 0; font-size: 1.1rem; }
  .modal p { font-size: 0.9rem; color: var(--text-dim); margin: 0 0 16px; line-height: 1.5; }
  .modal p code {
    background: rgba(255, 255, 255, 0.1);
    padding: 1px 6px; border-radius: 3px; font-size: 0.85rem;
  }
  .error-banner {
    background: rgba(239, 68, 68, 0.2);
    border: 1px solid var(--error);
    color: var(--error);
    padding: 10px 14px; border-radius: 8px; margin-bottom: 12px;
    font-size: 0.85rem;
  }
  .modal-actions {
    display: flex; justify-content: flex-end; gap: 10px;
  }
  .danger {
    background: var(--error);
    color: white;
  }
  .danger:hover { opacity: 0.9; }
</style>
