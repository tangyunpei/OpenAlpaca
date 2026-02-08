<script lang="ts">
  import type { ChatMessage } from "$lib/types";

  interface Props {
    message: ChatMessage;
  }

  let { message }: Props = $props();

  function formatTime(ts: string): string {
    try {
      const d = new Date(ts);
      return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    } catch {
      return "";
    }
  }

  /** Minimal markdown: **bold**, *italic*, `code`, and newlines to <br>. */
  function renderMarkdown(text: string): string {
    return text
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
      .replace(/\*(.+?)\*/g, "<em>$1</em>")
      .replace(/`(.+?)`/g, "<code>$1</code>")
      .replace(/\n/g, "<br>");
  }

  let renderedContent = $derived(renderMarkdown(message.content));
</script>

<div class="flex mb-3 px-2 {message.role === 'user' ? 'justify-end' : 'justify-start'}">
  <div class="max-w-[85%] px-3.5 py-2.5 rounded-xl text-sm leading-relaxed break-words {message.role === 'user' ? 'bg-primary text-foreground rounded-br-sm' : message.role === 'assistant' ? 'bg-card text-foreground rounded-bl-sm' : 'bg-white/5 text-muted-foreground italic max-w-full text-center'}">
    <div class={message.content === "Thinking..." ? "text-muted-foreground animate-thinking" : ""}>
      {@html renderedContent}
    </div>
    <div class="flex gap-2 mt-1.5 text-[0.7rem] text-muted-foreground flex-wrap">
      {#if message.model}
        <span class="bg-white/10 px-1.5 rounded font-mono">{message.model}</span>
      {/if}
      {#if message.tokens_in || message.tokens_out}
        <span class="opacity-70">{message.tokens_in}/{message.tokens_out} tok</span>
      {/if}
      {#if message.duration_ms}
        <span class="opacity-70">{message.duration_ms}ms</span>
      {/if}
      <span>{formatTime(message.created_at)}</span>
    </div>
  </div>
</div>
