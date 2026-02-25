<script lang="ts">
  import { connectToDaemon } from "$lib/daemon";
  import { Button } from "$lib/ui";

  interface Props {
    statusState: string;
    info: { baseUrl: string; instanceId: string } | null;
    onToggleSettings?: () => void;
  }

  let { statusState, info, onToggleSettings }: Props = $props();
</script>

<header class="flex justify-between items-center mb-5 pb-4 shrink-0 flex-wrap gap-3 relative">
  <!-- Brand + instance info -->
  <div class="flex items-center gap-4 min-w-0">
    <!-- Logo mark -->
    <div class="w-9 h-9 rounded-xl flex items-center justify-center shrink-0"
         style="background: linear-gradient(135deg, hsl(40 85% 58% / 0.15) 0%, hsl(40 85% 58% / 0.05) 100%); border: 1px solid hsl(40 85% 58% / 0.15);">
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-accent">
        <path d="M12 2L2 7l10 5 10-5-10-5z"/><path d="M2 17l10 5 10-5"/><path d="M2 12l10 5 10-5"/>
      </svg>
    </div>
    <div class="flex flex-col gap-0.5 min-w-0">
      <h1 class="m-0 text-xl font-bold text-foreground tracking-tight max-[480px]:text-lg" style="letter-spacing: -0.02em;">
        OpenAlpaca
      </h1>
      {#if info}
        <div class="flex gap-2 items-center font-mono text-[0.68rem] text-muted-foreground">
          <span class="px-1.5 py-px rounded-md text-foreground/80" style="background: linear-gradient(135deg, hsl(222 48% 22%) 0%, hsl(222 48% 18%) 100%);">{info.instanceId.slice(0, 8)}</span>
          <span class="opacity-60">{info.baseUrl}</span>
        </div>
      {/if}
    </div>
  </div>

  <!-- Controls -->
  <div class="flex items-center gap-2.5">
    {#if onToggleSettings}
      <button
        class="flex items-center justify-center w-9 h-9 rounded-xl bg-white/[0.04] text-muted-foreground hover:bg-accent/15 hover:text-accent transition-all duration-200 cursor-pointer border border-white/5 hover:border-accent/20"
        onclick={onToggleSettings}
        title="Settings"
        aria-label="Open settings panel"
      >
        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/>
          <circle cx="12" cy="12" r="3"/>
        </svg>
      </button>
    {/if}

    <!-- Status pill -->
    <div class="flex items-center gap-2 px-3.5 py-1.5 rounded-full text-xs capitalize border border-white/5"
         style="background: linear-gradient(135deg, rgba(255,255,255,0.04) 0%, rgba(255,255,255,0.02) 100%);">
      <span class="relative flex items-center justify-center w-2 h-2">
        <span class="w-2 h-2 rounded-full {statusState === 'connected' ? 'bg-success' : statusState === 'error' ? 'bg-danger' : 'bg-muted-foreground animate-pulse'}"></span>
        {#if statusState === 'connected'}
          <span class="absolute inset-0 rounded-full bg-success/40 animate-ping" style="animation-duration: 2s;"></span>
        {/if}
      </span>
      <span class="text-foreground/80 font-medium">{statusState}</span>
    </div>

    <Button
      variant="secondary"
      size="sm"
      onclick={() => connectToDaemon()}
      disabled={statusState === "connecting"}
    >
      {statusState === "connecting"
        ? "Connecting..."
        : statusState === "disconnected"
          ? "Connect"
          : "Reconnect"}
    </Button>
  </div>

  <!-- Bottom border gradient -->
  <div class="absolute bottom-0 left-0 right-0 h-px"
       style="background: linear-gradient(90deg, transparent, hsl(222 48% 22%) 20%, hsl(40 85% 58% / 0.2) 50%, hsl(222 48% 22%) 80%, transparent);"></div>
</header>
