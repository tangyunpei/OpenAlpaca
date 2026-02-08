<script lang="ts">
  import { reorderKeys, setKeyPriority } from "$lib/api/settings";
  import type { KeyInfo } from "$lib/types";
  import DeleteKeyDialog from "./DeleteKeyDialog.svelte";

  interface Props {
    provider: string;
    keys: KeyInfo[];
    onRefresh: () => void;
  }

  let { provider, keys, onRefresh }: Props = $props();

  let draggedIndex = $state<number | null>(null);
  let dragOverIndex = $state<number | null>(null);

  function handleDragStart(e: DragEvent, index: number) {
    // Some browsers require drag data to be set to initiate DnD reliably.
    e.dataTransfer?.setData("text/plain", keys[index]?.id ?? String(index));
    e.dataTransfer?.setDragImage(new Image(), 0, 0);
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
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

    const newOrder = [...keys];
    const [moved] = newOrder.splice(draggedIndex, 1);
    newOrder.splice(targetIndex, 0, moved);

    try {
      await reorderKeys({
        provider,
        key_order: newOrder.map(k => k.id),
      });
      onRefresh();
    } catch (e) {
      console.error("Reorder failed:", e);
    }

    handleDragEnd();
  }

  async function handleTogglePriority(keyId: string, currentPriority: string) {
    const nextPriority = currentPriority === "fallback" ? "primary" : "fallback";

    try {
      await setKeyPriority({
        provider,
        key_id: keyId,
        priority: nextPriority,
      });
      onRefresh();
    } catch (e) {
      console.error("Set priority failed:", e);
    }
  }

  let pendingDeleteKeyId = $state<string | null>(null);

  function handleDelete(keyId: string) {
    pendingDeleteKeyId = keyId;
  }

  function healthDotClass(status: string): string {
    switch (status) {
      case "healthy": return "bg-success";
      case "rate_limited": return "bg-amber-500";
      case "error": return "bg-danger";
      default: return "bg-muted-foreground";
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
      case "active": return "bg-success/20 text-success";
      case "expired": return "bg-amber-500/20 text-amber-500";
      case "refreshing": return "bg-blue-500/20 text-blue-500 animate-pulse";
      default: return "bg-gray-500/20 text-gray-400";
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

<div class="flex flex-col gap-2">
  {#each keys as key, index (key.id)}
    <div
      class="flex items-center gap-2.5 px-3.5 py-3 bg-white/3 rounded-lg border border-white/5 transition-all hover:bg-white/6 hover:border-white/10 {draggedIndex === index ? 'opacity-40' : ''} {dragOverIndex === index ? 'border-accent bg-accent/5' : ''}"
      ondragover={(e) => handleDragOver(e, index)}
      ondrop={() => handleDrop(index)}
      role="listitem"
    >
      <button
        type="button"
        class="cursor-grab text-muted-foreground opacity-40 shrink-0 hover:opacity-100 bg-transparent border-none p-0"
        draggable="true"
        ondragstart={(e) => handleDragStart(e, index)}
        ondragend={handleDragEnd}
        title="Drag to reorder"
        aria-label="Drag to reorder"
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor">
          <circle cx="9" cy="6" r="1.5" /><circle cx="15" cy="6" r="1.5" />
          <circle cx="9" cy="12" r="1.5" /><circle cx="15" cy="12" r="1.5" />
          <circle cx="9" cy="18" r="1.5" /><circle cx="15" cy="18" r="1.5" />
        </svg>
      </button>

      <button
        class="bg-transparent border-none cursor-pointer text-xl p-0.5 shrink-0 leading-none {key.priority === 'primary' ? 'text-amber-500' : 'text-muted-foreground'} hover:scale-110 transition-transform"
        onclick={() => handleTogglePriority(key.id, key.priority)}
        title={key.priority === "primary" ? "Set as fallback" : "Set as primary"}
      >
        {key.priority === "primary" ? "\u2605" : "\u2606"}
      </button>

      <div class="flex-1 min-w-0">
        <div class="flex items-center gap-1.5 mb-0.5">
          <span class="font-semibold text-sm text-foreground">{key.id}</span>
          <span class="w-2 h-2 rounded-full shrink-0 {healthDotClass(key.status)}" title={key.status}></span>
        </div>
        <div class="flex items-center gap-1.5 flex-wrap">
          <code class="text-xs text-muted-foreground bg-white/5 px-1.5 py-px rounded">{key.masked_secret}</code>
          <span class="text-[0.65rem] px-1.5 py-px rounded uppercase font-semibold bg-primary/40 text-muted-foreground">{sourceLabel(key.source)}</span>
          {#if key.tier}
            <span class="text-[0.65rem] px-1.5 py-px rounded uppercase font-semibold bg-primary/40 text-muted-foreground">{key.tier}</span>
          {/if}
        </div>
        {#if key.notes}
          <div class="text-xs text-muted-foreground mt-0.5 opacity-70">{key.notes}</div>
        {/if}
        {#if isManagedKey(key)}
          <div class="flex items-center gap-1.5 mt-1 flex-wrap">
            {#if key.credential_status}
              <span class="text-[0.6rem] px-1.5 py-px rounded uppercase font-bold {credentialStatusClass(key.credential_status)}">
                {key.credential_status}
              </span>
            {/if}
            {#if key.credential_expires_at}
              <span class="text-[0.65rem] text-muted-foreground">{formatExpiryCountdown(key.credential_expires_at)}</span>
            {/if}
            {#if key.managed}
              <span class="text-[0.6rem] px-1.5 py-px rounded uppercase font-semibold bg-violet-500/20 text-violet-400">Auto-detected</span>
            {/if}
          </div>
          {#if key.external_usage}
            <div class="flex items-center gap-2.5 mt-1 flex-wrap">
              <span class="flex items-center gap-1 text-[0.7rem]">
                <span class="text-muted-foreground">Cost:</span>
                <span class="text-foreground font-mono font-semibold">${key.external_usage.cost_usd.toFixed(2)}</span>
              </span>
              <span class="flex items-center gap-1 text-[0.7rem]">
                <span class="text-muted-foreground">Tokens:</span>
                <span class="text-foreground font-mono font-semibold">{formatTokenCount(key.external_usage.token_count)}</span>
              </span>
              {#if key.external_usage.rate_limit_remaining != null}
                <span class="flex items-center gap-1 text-[0.7rem]">
                  <span class="text-muted-foreground">Rate limit:</span>
                  <span class="text-foreground font-mono font-semibold">{key.external_usage.rate_limit_remaining}</span>
                </span>
              {/if}
              {#if key.external_usage.approximate}
                <span class="text-[0.55rem] px-1 rounded bg-amber-500/15 text-amber-500 uppercase">approx</span>
              {/if}
            </div>
          {/if}
        {/if}
      </div>

      {#if isManagedKey(key)}
        <span class="text-muted-foreground opacity-40 shrink-0 p-1" title="Auto-managed credential (non-deletable)">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <rect x="3" y="11" width="18" height="11" rx="2" ry="2"></rect>
            <path d="M7 11V7a5 5 0 0 1 10 0v4"></path>
          </svg>
        </span>
      {:else}
        <button
          class="bg-transparent border-none cursor-pointer text-muted-foreground p-1 shrink-0 opacity-50 transition-all hover:text-danger hover:opacity-100"
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
    <div class="text-muted-foreground text-center py-6 text-sm">No API keys configured for this provider.</div>
  {/each}
</div>

{#if pendingDeleteKeyId}
  <DeleteKeyDialog
    keyId={pendingDeleteKeyId}
    {provider}
    onClose={() => pendingDeleteKeyId = null}
    onDeleted={() => { pendingDeleteKeyId = null; onRefresh(); }}
  />
{/if}
