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

<div class="chat-message" class:user={message.role === "user"} class:assistant={message.role === "assistant"} class:system={message.role === "system"}>
  <div class="bubble">
    <div class="content" class:thinking={message.content === "Thinking..."}>
      {@html renderedContent}
    </div>
    <div class="meta">
      {#if message.model}
        <span class="model-badge">{message.model}</span>
      {/if}
      {#if message.tokens_in || message.tokens_out}
        <span class="tokens">{message.tokens_in}/{message.tokens_out} tok</span>
      {/if}
      {#if message.duration_ms}
        <span class="duration">{message.duration_ms}ms</span>
      {/if}
      <span class="time">{formatTime(message.created_at)}</span>
    </div>
  </div>
</div>

<style>
  .chat-message {
    display: flex;
    margin-bottom: 12px;
    padding: 0 8px;
  }

  .chat-message.user {
    justify-content: flex-end;
  }

  .chat-message.assistant,
  .chat-message.system {
    justify-content: flex-start;
  }

  .bubble {
    max-width: 85%;
    padding: 10px 14px;
    border-radius: 12px;
    font-size: 0.9rem;
    line-height: 1.5;
    word-break: break-word;
  }

  .user .bubble {
    background: var(--primary);
    color: var(--text);
    border-bottom-right-radius: 4px;
  }

  .assistant .bubble {
    background: var(--surface);
    color: var(--text);
    border-bottom-left-radius: 4px;
  }

  .system .bubble {
    background: rgba(255, 255, 255, 0.05);
    color: var(--text-dim);
    font-style: italic;
    max-width: 100%;
    text-align: center;
  }

  .content.thinking {
    color: var(--text-dim);
    animation: pulse 1.5s ease-in-out infinite;
  }

  .meta {
    display: flex;
    gap: 8px;
    margin-top: 6px;
    font-size: 0.7rem;
    color: var(--text-dim);
    flex-wrap: wrap;
  }

  .model-badge {
    background: rgba(255, 255, 255, 0.1);
    padding: 1px 6px;
    border-radius: 4px;
    font-family: "Fira Code", monospace;
  }

  .tokens, .duration {
    opacity: 0.7;
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.4; }
  }
</style>
