<script lang="ts">
  import type { ChatMessage, AttachmentDisplay, Citation, Artifact } from "$lib/types";
  import { formatFileSize } from "$lib/types";
  import { renderMarkdown } from "$lib/markdown";
  import { downloadFile } from "$lib/api/files";

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

  async function handleDownload(att: AttachmentDisplay) {
    try {
      const blob = await downloadFile(att.file_id);
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = att.filename;
      a.click();
      URL.revokeObjectURL(url);
    } catch (e) {
      console.error("[ChatMessage] download failed:", e);
    }
  }

  function mimeIcon(mime: string): string {
    if (mime.startsWith("image/")) return "img";
    if (mime.startsWith("audio/")) return "audio";
    if (mime === "application/pdf") return "pdf";
    if (mime.startsWith("text/")) return "text";
    return "file";
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

        {#if message.attachments && message.attachments.length > 0}
          <div class="flex flex-wrap gap-1.5 mt-2 pt-2 border-t border-white/[0.06]">
            {#each message.attachments as att}
              <button
                class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-xs border border-white/8 bg-white/[0.03]
                       hover:bg-white/[0.06] hover:border-white/12 transition-all cursor-pointer"
                onclick={() => handleDownload(att)}
                title="Download {att.filename}"
              >
                <!-- File type icon -->
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="shrink-0 text-muted-foreground">
                  {#if mimeIcon(att.mime_type) === "img"}
                    <rect width="18" height="18" x="3" y="3" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>
                  {:else if mimeIcon(att.mime_type) === "pdf"}
                    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8Z"/><path d="M14 2v6h6"/>
                  {:else if mimeIcon(att.mime_type) === "audio"}
                    <path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>
                  {:else}
                    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8Z"/><path d="M14 2v6h6"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/>
                  {/if}
                </svg>
                <span class="truncate max-w-[100px]">{att.filename}</span>
                <span class="text-muted-foreground/50 shrink-0">{formatFileSize(att.size_bytes)}</span>
              </button>
            {/each}
          </div>
        {/if}

        {#if message.citations && message.citations.length > 0}
          <div class="flex flex-col gap-1 mt-2 pt-2 border-t border-white/[0.06]">
            <span class="text-[0.65rem] text-muted-foreground/50 uppercase tracking-wider font-medium">Sources</span>
            {#each message.citations as cit, i}
              <div class="flex items-start gap-1.5 text-xs text-muted-foreground">
                <span class="shrink-0 text-accent/60 font-mono">[{i + 1}]</span>
                <span class="break-words">{cit.excerpt}{#if cit.page} <span class="text-muted-foreground/40">(p.{cit.page})</span>{/if}</span>
              </div>
            {/each}
          </div>
        {/if}

        {#if message.artifacts && message.artifacts.length > 0}
          <div class="flex flex-wrap gap-1.5 mt-2 pt-2 border-t border-white/[0.06]">
            {#each message.artifacts as art}
              <button
                class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-xs border border-accent/15 bg-accent/[0.04]
                       hover:bg-accent/[0.08] hover:border-accent/25 transition-all cursor-pointer"
                onclick={() => handleDownload({ file_id: art.file_id, filename: art.label, mime_type: art.mime_type, size_bytes: 0 })}
                title="Download {art.label}"
              >
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="shrink-0 text-accent/60">
                  <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/>
                </svg>
                <span class="truncate max-w-[120px]">{art.label}</span>
              </button>
            {/each}
          </div>
        {/if}

        <!-- Copy button (appears on hover) -->
        {#if !isThinking}
          <button
            class="absolute -bottom-2.5 {message.role === 'user' ? 'left-1' : 'right-1'} z-10 opacity-0 group-hover:opacity-100
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
