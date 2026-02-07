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
      case "codex": return "Codex";
      case "claude_max_pro": return "Max/Pro";
      case "environment": return "Env Var";
      default: return source;
    }
  }

  function isManagedKey(key: KeyInfo): boolean {
    return key.managed === true || key.source === "claude_code" || key.source === "codex";
  }

  function credentialStatusClass(status: string | null | undefined): string {
    switch (status) {
      case "active": return "cred-active";
      case "expired": return "cred-expired";
      case "refreshing": return "cred-refreshing";
      default: return "cred-unknown";
    }
  }

  function formatTokenCount(count: number): string {
    if (count >= 1_000_000) return `${(count / 1_000_000).toFixed(1)}M`;
    if (count >= 1_000) return `${(count / 1_000).toFixed(1)}K`;
    return String(count);
  }

  function formatExpiryCountdown(expiresAt: number | null | undefined): string {
    if (!expiresAt) return "";
    const now = Date.now() / 1000;
    const remaining = expiresAt - now;
    if (remaining <= 0) return "Expired";
    const hours = Math.floor(remaining / 3600);
    const minutes = Math.floor((remaining % 3600) / 60);
    if (hours > 0) return `Expires in ${hours}h ${minutes}m`;
    return `Expires in ${minutes}m`;
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
        {#if isManagedKey(key)}
          <div class="credential-row">
            {#if key.credential_status}
              <span class="cred-badge {credentialStatusClass(key.credential_status)}">
                {key.credential_status}
              </span>
            {/if}
            {#if key.credential_expires_at}
              <span class="cred-expiry">{formatExpiryCountdown(key.credential_expires_at)}</span>
            {/if}
            {#if key.managed}
              <span class="auto-badge">Auto-detected</span>
            {/if}
          </div>
          {#if key.external_usage}
            <div class="usage-row">
              <span class="usage-item">
                <span class="usage-label">Cost:</span>
                <span class="usage-value">${key.external_usage.cost_usd.toFixed(2)}</span>
              </span>
              <span class="usage-item">
                <span class="usage-label">Tokens:</span>
                <span class="usage-value">{formatTokenCount(key.external_usage.token_count)}</span>
              </span>
              {#if key.external_usage.rate_limit_remaining != null}
                <span class="usage-item">
                  <span class="usage-label">Rate limit:</span>
                  <span class="usage-value">{key.external_usage.rate_limit_remaining}</span>
                </span>
              {/if}
              {#if key.external_usage.approximate}
                <span class="approx-badge">approx</span>
              {/if}
            </div>
          {/if}
        {/if}
      </div>

      {#if isManagedKey(key)}
        <span class="managed-lock" title="Auto-managed credential (non-deletable)">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <rect x="3" y="11" width="18" height="11" rx="2" ry="2"></rect>
            <path d="M7 11V7a5 5 0 0 1 10 0v4"></path>
          </svg>
        </span>
      {:else}
        <button
          class="delete-btn"
          onclick={() => handleDelete(key.id)}
          title="Delete key"
        >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M3 6h18m-2 0v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" />
        </svg>
      </button>
      {/if}
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

  .credential-row {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 4px;
    flex-wrap: wrap;
  }

  .cred-badge {
    font-size: 0.6rem;
    padding: 1px 6px;
    border-radius: 3px;
    text-transform: uppercase;
    font-weight: 700;
  }

  .cred-active {
    background: rgba(16, 185, 129, 0.2);
    color: var(--success);
  }

  .cred-expired {
    background: rgba(245, 158, 11, 0.2);
    color: #f59e0b;
  }

  .cred-refreshing {
    background: rgba(59, 130, 246, 0.2);
    color: #3b82f6;
    animation: pulse 1.5s ease-in-out infinite;
  }

  .cred-unknown {
    background: rgba(107, 114, 128, 0.2);
    color: #9ca3af;
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.5; }
  }

  .cred-expiry {
    font-size: 0.65rem;
    color: var(--text-dim);
  }

  .auto-badge {
    font-size: 0.6rem;
    padding: 1px 6px;
    border-radius: 3px;
    background: rgba(139, 92, 246, 0.2);
    color: #a78bfa;
    text-transform: uppercase;
    font-weight: 600;
  }

  .usage-row {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 4px;
    flex-wrap: wrap;
  }

  .usage-item {
    display: flex;
    align-items: center;
    gap: 3px;
    font-size: 0.7rem;
  }

  .usage-label {
    color: var(--text-dim);
  }

  .usage-value {
    color: var(--text);
    font-family: monospace;
    font-weight: 600;
  }

  .approx-badge {
    font-size: 0.55rem;
    padding: 0px 4px;
    border-radius: 2px;
    background: rgba(245, 158, 11, 0.15);
    color: #f59e0b;
    text-transform: uppercase;
  }

  .managed-lock {
    color: var(--text-dim);
    opacity: 0.4;
    flex-shrink: 0;
    padding: 4px;
  }
</style>
