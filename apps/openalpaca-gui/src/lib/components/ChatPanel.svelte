<script lang="ts">
  import { onMount, onDestroy, tick } from "svelte";
  import ChatMessageComponent from "./ChatMessage.svelte";
  import {
    chatMessages,
    chatLoading,
    chatError,
    chatStreaming,
    loadHistory,
    sendChatMessage,
    clearHistory,
    subscribeToChatEvents,
    subscribeToTaskResultEvents,
  } from "$lib/stores/chat";
  import { connectionState } from "$lib/daemon";
  import type { ChatMessage } from "$lib/types";

  let messages = $state<ChatMessage[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let streaming = $state(false);
  let historyLoaded = false;

  let inputText = $state("");
  let messagesContainer: HTMLDivElement | undefined = $state();

  const unsubMessages = chatMessages.subscribe((v) => {
    messages = v;
    scrollToBottom();
  });
  const unsubLoading = chatLoading.subscribe((v) => (loading = v));
  const unsubError = chatError.subscribe((v) => (error = v));
  const unsubStreaming = chatStreaming.subscribe((v) => (streaming = v));
  const unsubConnection = connectionState.subscribe((v) => {
    if (v === "connected" && !historyLoaded) {
      historyLoaded = true;
      loadHistory();
    }
  });
  let unsubChatEvents: (() => void) | null = null;
  let unsubTaskEvents: (() => void) | null = null;

  onMount(() => {
    unsubChatEvents = subscribeToChatEvents();
    unsubTaskEvents = subscribeToTaskResultEvents();
  });

  onDestroy(() => {
    unsubMessages();
    unsubLoading();
    unsubError();
    unsubStreaming();
    unsubConnection();
    unsubChatEvents?.();
    unsubTaskEvents?.();
  });

  async function scrollToBottom() {
    await tick();
    if (messagesContainer) {
      messagesContainer.scrollTop = messagesContainer.scrollHeight;
    }
  }

  function handleSend() {
    const text = inputText.trim();
    if (!text || streaming) return;
    inputText = "";
    sendChatMessage(text);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  }

  function handleClear() {
    clearHistory();
  }
</script>

<div class="flex flex-col h-full bg-background">
  <div class="flex justify-between items-center px-4 py-3 border-b border-primary shrink-0">
    <h3 class="m-0 text-base font-semibold text-foreground">Chat</h3>
    <button
      class="px-3 py-1 text-xs bg-white/5 text-muted-foreground border border-input rounded-md cursor-pointer hover:bg-danger/20 hover:text-danger hover:border-danger transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
      onclick={handleClear}
      disabled={messages.length === 0}
    >
      Clear
    </button>
  </div>

  {#if error}
    <div class="bg-danger/15 text-danger px-4 py-2 text-xs shrink-0">{error}</div>
  {/if}

  <div class="flex-1 overflow-y-auto py-3 min-h-0" bind:this={messagesContainer}>
    {#if messages.length === 0 && !loading}
      <div class="flex items-center justify-center h-full text-muted-foreground text-sm px-5 text-center">
        <p>Send a message to start chatting with OpenAlpaca.</p>
      </div>
    {:else}
      {#each messages as msg (msg.id)}
        <ChatMessageComponent message={msg} />
      {/each}
    {/if}
  </div>

  <div class="flex gap-2 px-4 py-3 border-t border-primary shrink-0">
    <textarea
      bind:value={inputText}
      onkeydown={handleKeydown}
      placeholder="Type a message..."
      disabled={streaming}
      rows={1}
      class="flex-1 bg-card border border-input rounded-lg text-foreground px-3 py-2 text-sm font-[inherit] resize-none min-h-[36px] max-h-[120px] outline-none focus:border-accent disabled:opacity-50 transition-colors"
    ></textarea>
    <button
      class="px-4 py-2 text-sm bg-accent text-accent-foreground border-none rounded-lg cursor-pointer whitespace-nowrap self-end hover:brightness-115 disabled:opacity-40 disabled:cursor-not-allowed transition-all"
      onclick={handleSend}
      disabled={!inputText.trim() || streaming}
    >
      {#if streaming}
        ...
      {:else}
        Send
      {/if}
    </button>
  </div>
</div>
