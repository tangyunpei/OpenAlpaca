<script lang="ts">
  import type { ChatMessage } from "$lib/types";
  import { renderMarkdown } from "$lib/markdown";

  interface Props {
    message: ChatMessage;
  }

  let { message }: Props = $props();

  let copied = $state(false);

  function formatTime(ts: string): string {
    try {
      const d = new Date(ts);
      return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    } catch {
      return "";
    }
  }

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(message.content);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      // Fallback for environments where clipboard API isn't available
    }
  }

  let renderedContent = $derived(renderMarkdown(message.content));
  let isThinking = $derived(message.content === "Thinking...");
</script>

{#if message.role === "system"}
  <!-- System messages: centered, no avatar -->
  <div class="flex mb-3 px-2 justify-center">
    <div class="max-w-full px-3.5 py-2 rounded-xl text-sm leading-relaxed break-words bg-white/5 text-muted-foreground italic text-center">
      <div class="oa-markdown">
        {@html renderedContent}
      </div>
    </div>
  </div>
{:else}
  <!-- User / Assistant messages -->
  <div class="group flex mb-3 px-2 gap-2.5 {message.role === 'user' ? 'flex-row-reverse' : 'flex-row'}">
    <!-- Avatar -->
    <div class="shrink-0 w-7 h-7 rounded-lg flex items-center justify-center text-xs font-bold mt-0.5
                {message.role === 'user' ? 'bg-accent/20 text-accent' : 'bg-white/8 text-muted-foreground'}">
      {#if message.role === "user"}
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/>
          <circle cx="12" cy="7" r="4"/>
        </svg>
      {:else}
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/>
        </svg>
      {/if}
    </div>

    <!-- Bubble + metadata -->
    <div class="max-w-[80%] min-w-0">
      <div class="relative px-3.5 py-2.5 rounded-xl text-sm leading-relaxed break-words
                  {message.role === 'user' ? 'bg-primary text-foreground rounded-tr-sm' : 'bg-card text-foreground rounded-tl-sm'}">
        <div class="oa-markdown {isThinking ? 'text-muted-foreground animate-thinking' : ''}">
          {@html renderedContent}
        </div>

        <!-- Copy button (appears on hover) -->
        {#if !isThinking}
          <button
            class="absolute -bottom-1 {message.role === 'user' ? 'left-1' : 'right-1'} opacity-0 group-hover:opacity-100
                   flex items-center gap-1 px-1.5 py-0.5 rounded-md text-[0.6rem]
                   bg-background/80 backdrop-blur-sm border border-white/8 text-muted-foreground
                   hover:text-foreground hover:bg-background transition-all duration-150 cursor-pointer"
            onclick={handleCopy}
            aria-label="Copy message"
          >
            {#if copied}
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="20 6 9 17 4 12"/>
              </svg>
              Copied
            {:else}
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect width="14" height="14" x="8" y="8" rx="2"/>
                <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>
              </svg>
              Copy
            {/if}
          </button>
        {/if}
      </div>

      <!-- Metadata row -->
      <div class="flex gap-2 mt-1.5 text-[0.65rem] text-muted-foreground/70 flex-wrap {message.role === 'user' ? 'justify-end' : ''}">
        {#if message.model}
          <span class="bg-white/5 px-1.5 rounded font-mono">{message.model}</span>
        {/if}
        {#if message.tokens_in || message.tokens_out}
          <span>{message.tokens_in}/{message.tokens_out} tok</span>
        {/if}
        {#if message.duration_ms}
          <span>{message.duration_ms}ms</span>
        {/if}
        <span>{formatTime(message.created_at)}</span>
      </div>
    </div>
  </div>
{/if}
