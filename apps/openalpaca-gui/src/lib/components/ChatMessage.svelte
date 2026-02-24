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
  <!-- System messages: centered, subtle -->
  <div class="flex mb-4 px-3 justify-center">
    <div class="max-w-full px-4 py-2.5 rounded-xl text-[0.8rem] leading-relaxed break-words text-muted-foreground/80 italic text-center border border-white/[0.04]"
         style="background: linear-gradient(135deg, rgba(255,255,255,0.025) 0%, rgba(255,255,255,0.01) 100%);">
      <div class="oa-markdown">
        {@html renderedContent}
      </div>
    </div>
  </div>
{:else}
  <!-- User / Assistant messages -->
  <div class="group flex mb-4 px-3 gap-3 {message.role === 'user' ? 'flex-row-reverse' : 'flex-row'}">
    <!-- Avatar -->
    <div class="shrink-0 w-8 h-8 rounded-xl flex items-center justify-center mt-0.5 border
                {message.role === 'user'
                  ? 'border-accent/15 text-accent'
                  : 'border-white/5 text-muted-foreground'}"
         style={message.role === 'user'
           ? 'background: linear-gradient(135deg, hsl(40 85% 58% / 0.12) 0%, hsl(40 85% 58% / 0.04) 100%);'
           : 'background: linear-gradient(135deg, rgba(255,255,255,0.05) 0%, rgba(255,255,255,0.02) 100%);'}>
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
      <div class="relative px-4 py-3 rounded-2xl text-sm leading-relaxed break-words border
                  {message.role === 'user'
                    ? 'rounded-tr-md border-accent/10'
                    : 'rounded-tl-md border-white/[0.04]'}"
           style={message.role === 'user'
             ? 'background: linear-gradient(135deg, hsl(222 48% 22%) 0%, hsl(222 48% 19%) 100%);'
             : 'background: linear-gradient(135deg, hsl(226 20% 14%) 0%, hsl(226 20% 13%) 100%);'}>
        <div class="oa-markdown {isThinking ? 'text-muted-foreground animate-thinking' : 'text-foreground'}">
          {@html renderedContent}
        </div>

        <!-- Copy button (appears on hover) -->
        {#if !isThinking}
          <button
            class="absolute -bottom-1.5 {message.role === 'user' ? 'left-1' : 'right-1'} opacity-0 group-hover:opacity-100
                   flex items-center gap-1 px-2 py-1 rounded-lg text-[0.6rem] font-medium
                   bg-background/90 backdrop-blur-md border border-white/8 text-muted-foreground
                   hover:text-foreground hover:border-white/15 transition-all duration-200 cursor-pointer
                   shadow-sm"
            onclick={handleCopy}
            aria-label="Copy message"
          >
            {#if copied}
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" class="text-success">
                <polyline points="20 6 9 17 4 12"/>
              </svg>
              <span class="text-success">Copied</span>
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
      <div class="flex gap-2 mt-2 text-[0.62rem] text-muted-foreground/60 flex-wrap items-center {message.role === 'user' ? 'justify-end' : ''}">
        {#if message.model}
          <span class="px-1.5 py-px rounded-md font-mono border border-white/[0.04]"
                style="background: rgba(255,255,255,0.025);">{message.model}</span>
        {/if}
        {#if message.tokens_in || message.tokens_out}
          <span class="font-mono">{message.tokens_in}/{message.tokens_out} tok</span>
        {/if}
        {#if message.duration_ms}
          <span class="font-mono">{message.duration_ms}ms</span>
        {/if}
        <span>{formatTime(message.created_at)}</span>
      </div>
    </div>
  </div>
{/if}
