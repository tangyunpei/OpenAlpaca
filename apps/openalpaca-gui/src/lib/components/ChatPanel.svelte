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

<div class="chat-panel">
  <div class="chat-header">
    <h3>Chat</h3>
    <button class="clear-btn" onclick={handleClear} disabled={messages.length === 0}>
      Clear
    </button>
  </div>

  {#if error}
    <div class="chat-error">{error}</div>
  {/if}

  <div class="messages-area" bind:this={messagesContainer}>
    {#if messages.length === 0 && !loading}
      <div class="empty-state">
        <p>Send a message to start chatting with OpenAlpaca.</p>
      </div>
    {:else}
      {#each messages as msg (msg.id)}
        <ChatMessageComponent message={msg} />
      {/each}
    {/if}
  </div>

  <div class="input-area">
    <textarea
      bind:value={inputText}
      onkeydown={handleKeydown}
      placeholder="Type a message..."
      disabled={streaming}
      rows={1}
    ></textarea>
    <button
      class="send-btn"
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

<style>
  .chat-panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg);
  }

  .chat-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 16px;
    border-bottom: 1px solid var(--primary);
    flex-shrink: 0;
  }

  .chat-header h3 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
    color: var(--text);
  }

  .clear-btn {
    padding: 4px 12px !important;
    font-size: 0.75rem !important;
    background: rgba(255, 255, 255, 0.05) !important;
    color: var(--text-dim) !important;
    border: 1px solid rgba(255, 255, 255, 0.1) !important;
    border-radius: 6px !important;
    cursor: pointer;
  }

  .clear-btn:hover:not(:disabled) {
    background: rgba(239, 68, 68, 0.2) !important;
    color: var(--error) !important;
    border-color: var(--error) !important;
  }

  .chat-error {
    background: rgba(239, 68, 68, 0.15);
    color: var(--error);
    padding: 8px 16px;
    font-size: 0.8rem;
    flex-shrink: 0;
  }

  .messages-area {
    flex: 1;
    overflow-y: auto;
    padding: 12px 0;
    min-height: 0;
  }

  .empty-state {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-dim);
    font-size: 0.85rem;
    padding: 20px;
    text-align: center;
  }

  .input-area {
    display: flex;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--primary);
    flex-shrink: 0;
  }

  textarea {
    flex: 1;
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    color: var(--text);
    padding: 8px 12px;
    font-size: 0.85rem;
    font-family: inherit;
    resize: none;
    min-height: 36px;
    max-height: 120px;
    outline: none;
  }

  textarea:focus {
    border-color: var(--accent);
  }

  textarea:disabled {
    opacity: 0.5;
  }

  .send-btn {
    padding: 8px 16px !important;
    font-size: 0.85rem !important;
    background: var(--accent) !important;
    color: #fff !important;
    border: none !important;
    border-radius: 8px !important;
    cursor: pointer;
    white-space: nowrap;
    align-self: flex-end;
  }

  .send-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .send-btn:hover:not(:disabled) {
    filter: brightness(1.15);
  }
</style>
