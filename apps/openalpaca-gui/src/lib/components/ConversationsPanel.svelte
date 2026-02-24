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
      case "imessage": return "iM";
      case "gui": return "GUI";
      case "cli": return "CLI";
      default: return source.slice(0, 3).toUpperCase();
    }
  }

  function sourceColor(source: string): string {
    switch (source) {
      case "telegram": return "#2AABEE";
      case "imessage": return "#34C759";
      case "gui": return "#e94560";
      case "cli": return "#10b981";
      default: return "#8892b0";
    }
  }
</script>

<div class="flex flex-col gap-3">
  {#if selectedId}
    <!-- Message detail view -->
    <div class="flex items-center gap-3 pb-3 border-b border-border">
      <button
        class="px-3 py-1 text-[0.8rem] bg-white/5 border border-input rounded-md text-muted-foreground hover:bg-primary hover:text-foreground cursor-pointer transition-colors"
        onclick={handleBack}
      >Back</button>
      <span class="text-[0.95rem] font-semibold text-foreground flex-1">
        {convList.find((c) => c.id === selectedId)?.title || convList.find((c) => c.id === selectedId)?.lane_key || "Conversation"}
      </span>
      <span class="text-xs text-muted-foreground">{messagesTotal} messages</span>
    </div>

    <div class="max-h-[60vh] overflow-y-auto py-2">
      {#if msgLoading}
        <div class="text-muted-foreground text-center py-10 text-[0.9rem]">Loading messages...</div>
      {:else if messages.length === 0}
        <div class="text-muted-foreground text-center py-10 text-[0.9rem]">No messages in this conversation.</div>
      {:else}
        {#each messages as msg (msg.id)}
          <ChatMessageComponent message={msg} />
        {/each}
      {/if}
    </div>
  {:else}
    <!-- Conversations list view -->
    <div class="flex items-center gap-2.5">
      <button
        class="px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/80 disabled:opacity-50 cursor-pointer"
        onclick={() => loadConversations(filterSource === "all" ? undefined : filterSource)}
        disabled={loading}
      >
        {loading ? "Loading..." : "Refresh"}
      </button>
      <select
        bind:value={filterSource}
        onchange={handleFilterChange}
        class="bg-surface border border-input rounded-md text-foreground px-3 py-2 text-[0.85rem] cursor-pointer focus:outline-none focus:border-accent"
      >
        <option value="all">All Sources</option>
        <option value="gui">GUI</option>
        <option value="telegram">Telegram</option>
        <option value="imessage">iMessage</option>
        <option value="cli">CLI</option>
      </select>
    </div>

    {#if error}
      <div class="bg-danger/20 border border-danger text-danger px-3.5 py-2.5 rounded-lg text-[0.9rem]">{error}</div>
    {/if}

    <div class="flex flex-col gap-2">
      {#if loading && convList.length === 0}
        <div class="text-muted-foreground text-center py-10 text-[0.9rem]">Loading conversations...</div>
      {:else if convList.length === 0}
        <div class="text-muted-foreground text-center py-10 text-[0.9rem]">No conversations found.</div>
      {:else}
        {#each convList as conv (conv.id)}
          <button
            class="block w-full text-left bg-card/70 backdrop-blur-xl border border-border rounded-[10px] px-4 py-3 cursor-pointer transition-all hover:border-accent hover:bg-card/90 hover:-translate-y-px"
            onclick={() => selectConversation(conv)}
          >
            <div class="flex items-center gap-2 mb-1">
              <span
                class="inline-block px-2 py-0.5 rounded text-[0.65rem] font-bold text-white uppercase tracking-wide"
                style="background: {sourceColor(conv.source)}"
              >
                {sourceIcon(conv.source)}
              </span>
              <span class="text-[0.85rem] text-foreground font-mono flex-1">{conv.lane_key}</span>
              <span class="text-xs text-muted-foreground">{conv.message_count} msgs</span>
            </div>
            {#if conv.title}
              <div class="text-[0.85rem] text-foreground mb-1 font-medium">{conv.title}</div>
            {/if}
            <div class="flex gap-4 text-[0.72rem] text-muted-foreground">
              <span>Last: {formatTime(conv.last_message_at)}</span>
              <span>Created: {formatTime(conv.created_at)}</span>
            </div>
          </button>
        {/each}
      {/if}
    </div>
  {/if}
</div>
