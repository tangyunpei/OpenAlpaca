<script lang="ts">
  import { removeKey, reorderKeys } from "$lib/api/settings";
  import type { KeyInfo } from "$lib/types";

  interface Props {
    provider: string;
    keys: KeyInfo[];
    onRefresh: () => void;
  }

  let { provider, keys, onRefresh }: Props = $props();

  let draggedIndex = $state<number | null>(null);
  let dragOverIndex = $state<number | null>(null);

  function handleDragStart(index: number) {
    draggedIndex = index;
  }

  function handleDragOver(e: DragEvent, index: number) {
    e.preventDefault();
    dragOverIndex = index;
  }

  function handleDragEnd() {
    draggedIndex = null;
    dragOverIndex = null;
  }

  async function handleDrop(targetIndex: number) {
    if (draggedIndex === null || draggedIndex === targetIndex) {
      handleDragEnd();
      return;
    }

    // Build new order
    const newOrder = [...keys];
    const [moved] = newOrder.splice(draggedIndex, 1);
    newOrder.splice(targetIndex, 0, moved);

    const primaryKey = newOrder.find(k => k.priority === "primary");

    try {
      await reorderKeys({
        provider,
        key_order: newOrder.map(k => k.id),
        primary_key_id: primaryKey?.id,
      });
      onRefresh();
    } catch (e) {
      console.error("Reorder failed:", e);
    }

    handleDragEnd();
  }

  async function handleSetPrimary(keyId: string) {
    try {
      await reorderKeys({
        provider,
        key_order: keys.map(k => k.id),
        primary_key_id: keyId,
      });
      onRefresh();
    } catch (e) {
      console.error("Set primary failed:", e);
    }
  }

  async function handleDelete(keyId: string) {
    if (!confirm(`Remove key "${keyId}" from ${provider}?`)) return;
    try {
      await removeKey(provider, keyId);
      onRefresh();
    } catch (e) {
      console.error("Delete failed:", e);
    }
  }

  function healthDotClass(status: string): string {
    switch (status) {
      case "healthy": return "dot-healthy";
      case "rate_limited": return "dot-rate-limited";
      case "error": return "dot-error";
      default: return "dot-unknown";
    }
  }

  function sourceLabel(source: string): string {
    switch (source) {
      case "api_console": return "API Console";
      case "claude_code": return "Claude Code";
      case "claude_max_pro": return "Max/Pro";
      case "environment": return "Env Var";
      default: return source;
    }
  }
</script>

<div class="key-list">
  {#each keys as key, index (key.id)}
    <div
      class="key-card"
      class:dragging={draggedIndex === index}
      class:drag-over={dragOverIndex === index}
      draggable="true"
      ondragstart={() => handleDragStart(index)}
      ondragover={(e) => handleDragOver(e, index)}
      ondrop={() => handleDrop(index)}
      ondragend={handleDragEnd}
      role="listitem"
    >
      <div class="drag-handle" title="Drag to reorder">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor">
          <circle cx="9" cy="6" r="1.5" /><circle cx="15" cy="6" r="1.5" />
          <circle cx="9" cy="12" r="1.5" /><circle cx="15" cy="12" r="1.5" />
          <circle cx="9" cy="18" r="1.5" /><circle cx="15" cy="18" r="1.5" />
        </svg>
      </div>

      <button
        class="star-btn"
        class:primary={key.priority === "primary"}
        onclick={() => handleSetPrimary(key.id)}
        title={key.priority === "primary" ? "Primary key" : "Set as primary"}
      >
        {key.priority === "primary" ? "\u2605" : "\u2606"}
      </button>

      <div class="key-info">
        <div class="key-header">
          <span class="key-id">{key.id}</span>
          <span class="health-dot {healthDotClass(key.status)}" title={key.status}></span>
        </div>
        <div class="key-detail">
          <code class="masked-secret">{key.masked_secret}</code>
          <span class="source-badge">{sourceLabel(key.source)}</span>
          {#if key.tier}
            <span class="tier-badge">{key.tier}</span>
          {/if}
        </div>
        {#if key.notes}
          <div class="key-notes">{key.notes}</div>
        {/if}
      </div>

      <button
        class="delete-btn"
        onclick={() => handleDelete(key.id)}
        title="Delete key"
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M3 6h18m-2 0v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" />
        </svg>
      </button>
    </div>
  {:else}
    <div class="empty">No API keys configured for this provider.</div>
  {/each}
</div>

<style>
  .key-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .key-card {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px;
    background: rgba(255, 255, 255, 0.03);
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.05);
    transition: all 0.2s;
    cursor: default;
  }

  .key-card:hover {
    background: rgba(255, 255, 255, 0.06);
    border-color: rgba(255, 255, 255, 0.1);
  }

  .key-card.dragging {
    opacity: 0.4;
  }

  .key-card.drag-over {
    border-color: var(--accent);
    background: rgba(233, 69, 96, 0.05);
  }

  .drag-handle {
    cursor: grab;
    color: var(--text-dim);
    opacity: 0.4;
    flex-shrink: 0;
  }

  .drag-handle:hover {
    opacity: 1;
  }

  .star-btn {
    background: none !important;
    border: none !important;
    cursor: pointer;
    font-size: 1.2rem;
    padding: 2px !important;
    color: var(--text-dim);
    flex-shrink: 0;
    line-height: 1;
  }

  .star-btn.primary {
    color: #f59e0b;
  }

  .star-btn:hover {
    transform: scale(1.2) !important;
  }

  .key-info {
    flex: 1;
    min-width: 0;
  }

  .key-header {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 2px;
  }

  .key-id {
    font-weight: 600;
    font-size: 0.9rem;
    color: var(--text);
  }

  .health-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .dot-healthy { background: var(--success); }
  .dot-rate-limited { background: #f59e0b; }
  .dot-error { background: var(--error); }
  .dot-unknown { background: var(--text-dim); }

  .key-detail {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .masked-secret {
    font-size: 0.75rem;
    color: var(--text-dim);
    background: rgba(255, 255, 255, 0.05);
    padding: 1px 5px;
    border-radius: 3px;
  }

  .source-badge, .tier-badge {
    font-size: 0.65rem;
    padding: 1px 5px;
    border-radius: 3px;
    text-transform: uppercase;
    font-weight: 600;
    background: rgba(15, 52, 96, 0.4);
    color: var(--text-dim);
  }

  .key-notes {
    font-size: 0.75rem;
    color: var(--text-dim);
    margin-top: 2px;
    opacity: 0.7;
  }

  .delete-btn {
    background: none !important;
    border: none !important;
    cursor: pointer;
    color: var(--text-dim);
    padding: 4px !important;
    flex-shrink: 0;
    opacity: 0.5;
    transition: all 0.2s;
  }

  .delete-btn:hover {
    color: var(--error) !important;
    opacity: 1;
    transform: none !important;
  }

  .empty {
    color: var(--text-dim);
    text-align: center;
    padding: 24px;
    font-size: 0.9rem;
  }
</style>
