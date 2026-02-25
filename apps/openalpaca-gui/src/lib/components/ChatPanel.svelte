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
  let textareaEl: HTMLTextAreaElement | undefined = $state();
  let inputFocused = $state(false);

  /** Auto-resize textarea to fit content */
  function autoResize() {
    if (!textareaEl) return;
    textareaEl.style.height = "auto";
    textareaEl.style.height = Math.min(textareaEl.scrollHeight, 140) + "px";
  }

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
    if (textareaEl) textareaEl.style.height = "auto";
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

<div class="flex flex-col h-full">
  <!-- Chat header -->
  <div class="flex justify-between items-center px-4 py-3 shrink-0 relative">
    <div class="flex items-center gap-2.5">
      <h3 class="m-0 text-sm font-semibold text-foreground tracking-wide uppercase" style="letter-spacing: 0.04em;">Chat</h3>
      {#if messages.length > 0}
        <span class="text-[0.6rem] text-muted-foreground/60 font-mono">{messages.length} messages</span>
      {/if}
    </div>
    <button
      class="px-3 py-1.5 text-[0.7rem] bg-white/[0.03] text-muted-foreground border border-white/5 rounded-lg cursor-pointer font-medium tracking-wide
             hover:bg-danger/10 hover:text-danger hover:border-danger/30 transition-all duration-200
             disabled:opacity-40 disabled:cursor-not-allowed"
      onclick={handleClear}
      disabled={messages.length === 0}
    >
      Clear
    </button>
    <div class="absolute bottom-0 left-4 right-4 h-px" style="background: linear-gradient(90deg, transparent, var(--color-border-strong) 20%, var(--color-border-strong) 80%, transparent);"></div>
  </div>

  {#if error}
    <div class="bg-danger/10 text-danger px-4 py-2 text-xs shrink-0 border-b border-danger/20 flex items-center gap-2">
      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="shrink-0">
        <circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/>
      </svg>
      {error}
    </div>
  {/if}

  <!-- Messages area -->
  <div class="flex-1 overflow-y-auto py-4 min-h-0" bind:this={messagesContainer}>
    {#if messages.length === 0 && !loading}
      <div class="flex flex-col items-center justify-center h-full text-center px-8 animate-fadeIn">
        <!-- Empty state illustration -->
        <div class="w-16 h-16 mb-5 rounded-2xl flex items-center justify-center"
             style="background: linear-gradient(135deg, hsl(40 85% 58% / 0.1) 0%, hsl(40 85% 58% / 0.03) 100%); border: 1px solid hsl(40 85% 58% / 0.1);">
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class="text-accent/60">
            <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>
          </svg>
        </div>
        <p class="text-foreground/70 text-sm font-medium m-0">Start a conversation</p>
        <p class="text-muted-foreground/60 text-xs m-0 mt-1.5 max-w-[240px] leading-relaxed">Send a message to chat with OpenAlpaca. Use natural language for tasks, questions, or commands.</p>
      </div>
    {:else}
      {#each messages as msg, idx (msg.id)}
        <div style="animation: slideUp 0.3s cubic-bezier(0.16, 1, 0.3, 1) both; animation-delay: {Math.min(idx * 30, 300)}ms;">
          <ChatMessageComponent message={msg} />
        </div>
      {/each}
      {#if streaming}
        <div class="px-4 py-1 animate-fadeIn">
          <div class="oa-typing-dots">
            <span></span>
            <span></span>
            <span></span>
          </div>
        </div>
      {/if}
    {/if}
  </div>

  <!-- Input area -->
  <div class="px-4 py-3 shrink-0 relative">
    <div class="absolute top-0 left-4 right-4 h-px" style="background: linear-gradient(90deg, transparent, var(--color-border-strong) 20%, var(--color-border-strong) 80%, transparent);"></div>
    <div class="flex gap-2.5 items-end rounded-xl p-1.5 border transition-all duration-200
                {inputFocused ? 'border-accent/30 shadow-[0_0_0_3px_hsl(40_85%_58%/0.06)]' : 'border-white/8'}"
         style="background: linear-gradient(180deg, rgba(255,255,255,0.03) 0%, rgba(255,255,255,0.01) 100%);">
      <textarea
        bind:this={textareaEl}
        bind:value={inputText}
        onkeydown={handleKeydown}
        oninput={autoResize}
        onfocus={() => inputFocused = true}
        onblur={() => inputFocused = false}
        placeholder="Type a message..."
        disabled={streaming}
        rows={1}
        class="flex-1 bg-transparent border-none text-foreground px-2.5 py-2 text-sm font-[inherit] resize-none min-h-[36px] max-h-[140px] outline-none placeholder:text-muted-foreground/50 disabled:opacity-50"
      ></textarea>
      <button
        class="px-4 py-2 text-sm font-semibold border-none rounded-lg cursor-pointer whitespace-nowrap self-end transition-all duration-200
               disabled:opacity-30 disabled:cursor-not-allowed
               {!inputText.trim() || streaming
                 ? 'bg-white/5 text-muted-foreground'
                 : 'bg-accent text-accent-foreground hover:brightness-110 hover:shadow-[0_2px_8px_hsl(40_85%_58%/0.25)]'}"
        onclick={handleSend}
        disabled={!inputText.trim() || streaming}
      >
        {#if streaming}
          <span class="flex items-center gap-1.5">
            <span class="w-1.5 h-1.5 rounded-full bg-current animate-pulse"></span>
            Thinking
          </span>
        {:else}
          Send
        {/if}
      </button>
    </div>
  </div>
</div>
