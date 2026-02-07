<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import ChatMessageComponent from "./ChatMessage.svelte";
  import {
    conversations,
    conversationsLoading,
    conversationsError,
    selectedConversationId,
    selectedMessages,
    selectedMessagesTotal,
    messagesLoading,
    loadConversations,
    loadConversationMessages,
    clearSelection,
  } from "$lib/stores/conversations";
  import type { Conversation, ChatMessage } from "$lib/types";

  let convList = $state<Conversation[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let selectedId = $state<string | null>(null);
  let messages = $state<ChatMessage[]>([]);
  let messagesTotal = $state(0);
  let msgLoading = $state(false);
  let filterSource = $state<string>("all");

  const unsubConvs = conversations.subscribe((v) => (convList = v));
  const unsubLoading = conversationsLoading.subscribe((v) => (loading = v));
  const unsubError = conversationsError.subscribe((v) => (error = v));
  const unsubSelected = selectedConversationId.subscribe((v) => (selectedId = v));
  const unsubMessages = selectedMessages.subscribe((v) => (messages = v));
  const unsubTotal = selectedMessagesTotal.subscribe((v) => (messagesTotal = v));
  const unsubMsgLoading = messagesLoading.subscribe((v) => (msgLoading = v));

  onMount(() => {
    loadConversations();
  });

  onDestroy(() => {
    unsubConvs();
    unsubLoading();
    unsubError();
    unsubSelected();
    unsubMessages();
    unsubTotal();
    unsubMsgLoading();
  });

  function handleFilterChange() {
    const source = filterSource === "all" ? undefined : filterSource;
    loadConversations(source);
  }

  function selectConversation(conv: Conversation) {
    loadConversationMessages(conv.id);
  }

  function handleBack() {
    clearSelection();
  }

  function formatTime(ts: string | null): string {
    if (!ts) return "-";
    try {
      const d = new Date(ts);
      return d.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
    } catch {
      return ts;
    }
  }

  function sourceIcon(source: string): string {
    switch (source) {
      case "telegram": return "TG";
      case "gui": return "GUI";
      case "cli": return "CLI";
      default: return source.slice(0, 3).toUpperCase();
    }
  }

  function sourceColor(source: string): string {
    switch (source) {
      case "telegram": return "#2AABEE";
      case "gui": return "#e94560";
      case "cli": return "#10b981";
      default: return "#8892b0";
    }
  }
</script>

<div class="conversations-panel">
  {#if selectedId}
    <!-- Message detail view -->
    <div class="detail-header">
      <button class="back-btn" onclick={handleBack}>Back</button>
      <span class="detail-title">
        {convList.find((c) => c.id === selectedId)?.title || convList.find((c) => c.id === selectedId)?.lane_key || "Conversation"}
      </span>
      <span class="detail-count">{messagesTotal} messages</span>
    </div>

    <div class="messages-list">
      {#if msgLoading}
        <div class="loading">Loading messages...</div>
      {:else if messages.length === 0}
        <div class="empty">No messages in this conversation.</div>
      {:else}
        {#each messages as msg (msg.id)}
          <ChatMessageComponent message={msg} />
        {/each}
      {/if}
    </div>
  {:else}
    <!-- Conversations list view -->
    <div class="controls">
      <button onclick={() => loadConversations(filterSource === "all" ? undefined : filterSource)} disabled={loading}>
        {loading ? "Loading..." : "Refresh"}
      </button>
      <select bind:value={filterSource} onchange={handleFilterChange}>
        <option value="all">All Sources</option>
        <option value="gui">GUI</option>
        <option value="telegram">Telegram</option>
        <option value="cli">CLI</option>
      </select>
    </div>

    {#if error}
      <div class="panel-error">{error}</div>
    {/if}

    <div class="conv-list">
      {#if loading && convList.length === 0}
        <div class="loading">Loading conversations...</div>
      {:else if convList.length === 0}
        <div class="empty">No conversations found.</div>
      {:else}
        {#each convList as conv (conv.id)}
          <button class="conv-card" onclick={() => selectConversation(conv)}>
            <div class="conv-header">
              <span class="source-badge" style="background: {sourceColor(conv.source)}">
                {sourceIcon(conv.source)}
              </span>
              <span class="conv-lane">{conv.lane_key}</span>
              <span class="conv-count">{conv.message_count} msgs</span>
            </div>
            {#if conv.title}
              <div class="conv-title">{conv.title}</div>
            {/if}
            <div class="conv-meta">
              <span>Last: {formatTime(conv.last_message_at)}</span>
              <span>Created: {formatTime(conv.created_at)}</span>
            </div>
          </button>
        {/each}
      {/if}
    </div>
  {/if}
</div>

<style>
  .conversations-panel {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .controls {
    display: flex;
    gap: 10px;
    align-items: center;
  }

  .controls select {
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 6px;
    color: var(--text);
    padding: 8px 12px;
    font-size: 0.85rem;
    cursor: pointer;
  }
  .controls select:focus { outline: none; border-color: var(--accent); }
  .controls select option { background: var(--surface); color: var(--text); }

  .panel-error {
    background: rgba(239, 68, 68, 0.2);
    border: 1px solid var(--error);
    color: var(--error);
    padding: 10px 14px;
    border-radius: 8px;
    font-size: 0.9rem;
  }

  .conv-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .conv-card {
    display: block;
    width: 100%;
    text-align: left;
    background: rgba(30, 30, 50, 0.7) !important;
    backdrop-filter: blur(20px);
    -webkit-backdrop-filter: blur(20px);
    border: 1px solid rgba(255, 255, 255, 0.08) !important;
    border-radius: 10px !important;
    padding: 12px 16px !important;
    cursor: pointer;
    transition: all 0.2s;
  }

  .conv-card:hover {
    border-color: var(--accent) !important;
    background: rgba(30, 30, 50, 0.9) !important;
    transform: translateY(-1px) !important;
  }

  .conv-header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 4px;
  }

  .source-badge {
    display: inline-block;
    padding: 2px 8px;
    border-radius: 4px;
    font-size: 0.65rem;
    font-weight: 700;
    color: #fff;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .conv-lane {
    font-size: 0.85rem;
    color: var(--text);
    font-family: monospace;
    flex: 1;
  }

  .conv-count {
    font-size: 0.75rem;
    color: var(--text-dim);
  }

  .conv-title {
    font-size: 0.85rem;
    color: var(--text);
    margin-bottom: 4px;
    font-weight: 500;
  }

  .conv-meta {
    display: flex;
    gap: 16px;
    font-size: 0.72rem;
    color: var(--text-dim);
  }

  /* Detail view */
  .detail-header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding-bottom: 12px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  }

  .back-btn {
    padding: 4px 12px !important;
    font-size: 0.8rem !important;
    background: rgba(255, 255, 255, 0.05) !important;
    border: 1px solid rgba(255, 255, 255, 0.1) !important;
    border-radius: 6px !important;
    color: var(--text-dim) !important;
  }
  .back-btn:hover { background: var(--primary) !important; color: var(--text) !important; }

  .detail-title {
    font-size: 0.95rem;
    font-weight: 600;
    color: var(--text);
    flex: 1;
  }

  .detail-count {
    font-size: 0.75rem;
    color: var(--text-dim);
  }

  .messages-list {
    max-height: 60vh;
    overflow-y: auto;
    padding: 8px 0;
  }

  .loading, .empty {
    color: var(--text-dim);
    text-align: center;
    padding: 40px;
    font-size: 0.9rem;
  }
</style>
