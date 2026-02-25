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
    pendingFiles,
    uploadingFiles,
    addFiles,
    removePendingFile,
  } from "$lib/stores/chat";
  import type { PendingFile } from "$lib/stores/chat";
  import { get } from "svelte/store";
  import { connectionState } from "$lib/daemon";
  import type { ChatMessage } from "$lib/types";
  import { formatFileSize } from "$lib/types";

  let messages = $state<ChatMessage[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let streaming = $state(false);
  let historyLoaded = false;

  let inputText = $state("");
  let messagesContainer: HTMLDivElement | undefined = $state();
  let textareaEl: HTMLTextAreaElement | undefined = $state();
  let inputFocused = $state(false);
  let pending = $state<PendingFile[]>([]);
  let uploading = $state(false);
  let dragOver = $state(false);
  let fileInputEl: HTMLInputElement | undefined = $state();

  const unsubPending = pendingFiles.subscribe((v) => (pending = v));
  const unsubUploading = uploadingFiles.subscribe((v) => (uploading = v));

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
    unsubPending();
    unsubUploading();
  });

  async function scrollToBottom() {
    await tick();
    if (messagesContainer) {
      messagesContainer.scrollTop = messagesContainer.scrollHeight;
    }
  }

  function handleSend() {
    const text = inputText.trim();
    const hasPending = get(pendingFiles).length > 0;
    if ((!text && !hasPending) || streaming) return;
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

  function handleFileSelect() {
    fileInputEl?.click();
  }

  function handleFileInput(e: Event) {
    const input = e.target as HTMLInputElement;
    if (input.files && input.files.length > 0) {
      addFiles(input.files);
      input.value = ""; // reset so same file can be re-selected
    }
  }

  function handleDrop(e: DragEvent) {
    e.preventDefault();
    dragOver = false;
    if (e.dataTransfer?.files && e.dataTransfer.files.length > 0) {
      addFiles(e.dataTransfer.files);
    }
  }

  function handleDragOver(e: DragEvent) {
    e.preventDefault();
    dragOver = true;
  }

  function handleDragLeave() {
    dragOver = false;
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
  <div
    class="px-4 py-3 shrink-0 relative"
    ondragover={handleDragOver}
    ondragleave={handleDragLeave}
    ondrop={handleDrop}
  >
    <div class="absolute top-0 left-4 right-4 h-px" style="background: linear-gradient(90deg, transparent, var(--color-border-strong) 20%, var(--color-border-strong) 80%, transparent);"></div>

    {#if dragOver}
      <div class="absolute inset-0 bg-accent/10 border-2 border-dashed border-accent/40 rounded-xl z-10 flex items-center justify-center pointer-events-none">
        <span class="text-accent text-sm font-medium">Drop files here</span>
      </div>
    {/if}

    <!-- Pending files display -->
    {#if pending.length > 0}
      <div class="flex flex-wrap gap-2 mb-2">
        {#each pending as pf, idx}
          <div class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-xs border border-white/8 bg-white/[0.03]">
            <span class="truncate max-w-[120px]" title={pf.file.name}>{pf.file.name}</span>
            <span class="text-muted-foreground/60 shrink-0">{formatFileSize(pf.file.size)}</span>
            {#if uploading && pf.progress > 0 && pf.progress < 100}
              <span class="text-accent text-[0.6rem] font-mono shrink-0">{pf.progress}%</span>
            {/if}
            {#if !uploading}
              <button
                class="text-muted-foreground hover:text-danger transition-colors cursor-pointer p-0 bg-transparent border-none"
                onclick={() => removePendingFile(idx)}
                aria-label="Remove file"
              >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
                </svg>
              </button>
            {/if}
          </div>
        {/each}
      </div>
    {/if}

    <div class="flex gap-2.5 items-end rounded-xl p-1.5 border transition-all duration-200
                {inputFocused ? 'border-accent/30 shadow-[0_0_0_3px_hsl(40_85%_58%/0.06)]' : 'border-white/8'}"
         style="background: linear-gradient(180deg, rgba(255,255,255,0.03) 0%, rgba(255,255,255,0.01) 100%);">

      <!-- Hidden file input -->
      <input
        bind:this={fileInputEl}
        type="file"
        multiple
        class="hidden"
        oninput={handleFileInput}
      />

      <!-- Attach file button -->
      <button
        class="p-2 text-muted-foreground hover:text-foreground transition-colors cursor-pointer bg-transparent border-none self-end shrink-0
               disabled:opacity-30 disabled:cursor-not-allowed"
        onclick={handleFileSelect}
        disabled={streaming || uploading}
        aria-label="Attach files"
      >
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"/>
        </svg>
      </button>

      <textarea
        bind:this={textareaEl}
        bind:value={inputText}
        onkeydown={handleKeydown}
        oninput={autoResize}
        onfocus={() => inputFocused = true}
        onblur={() => inputFocused = false}
        placeholder={pending.length > 0 ? "Add a message about the files..." : "Type a message..."}
        disabled={streaming || uploading}
        rows={1}
        class="flex-1 bg-transparent border-none text-foreground px-2.5 py-2 text-sm font-[inherit] resize-none min-h-[36px] max-h-[140px] outline-none placeholder:text-muted-foreground/50 disabled:opacity-50"
      ></textarea>

      <button
        class="px-4 py-2 text-sm font-semibold border-none rounded-lg cursor-pointer whitespace-nowrap self-end transition-all duration-200
               disabled:opacity-30 disabled:cursor-not-allowed
               {!inputText.trim() && pending.length === 0 || streaming || uploading
                 ? 'bg-white/5 text-muted-foreground'
                 : 'bg-accent text-accent-foreground hover:brightness-110 hover:shadow-[0_2px_8px_hsl(40_85%_58%/0.25)]'}"
        onclick={handleSend}
        disabled={(!inputText.trim() && pending.length === 0) || streaming || uploading}
      >
        {#if uploading}
          <span class="flex items-center gap-1.5">
            <span class="w-1.5 h-1.5 rounded-full bg-current animate-pulse"></span>
            Uploading
          </span>
        {:else if streaming}
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
