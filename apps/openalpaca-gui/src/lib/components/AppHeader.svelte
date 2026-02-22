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

<header class="flex justify-between items-center mb-6 pb-4 border-b-2 border-primary flex-wrap gap-2.5 shrink-0">
  <div class="flex flex-col gap-1 min-w-0">
    <h1 class="m-0 text-2xl font-extrabold text-foreground tracking-tight max-[480px]:text-xl">
      OpenAlpaca
    </h1>
    {#if info}
      <div class="flex gap-3 font-mono text-xs text-muted-foreground flex-wrap">
        <span class="bg-primary px-1.5 rounded text-foreground">{info.instanceId.slice(0, 8)}</span>
        <span>{info.baseUrl}</span>
      </div>
    {/if}
  </div>
  <div class="flex items-center gap-2">
    {#if onToggleSettings}
      <button
        class="flex items-center justify-center w-9 h-9 rounded-lg bg-white/5 text-muted-foreground hover:bg-accent/20 hover:text-accent transition-all cursor-pointer border-none"
        onclick={onToggleSettings}
        title="Settings"
        aria-label="Open settings panel"
      >
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/>
          <circle cx="12" cy="12" r="3"/>
        </svg>
      </button>
    {/if}
    <div class="flex items-center gap-2 px-4 py-2 rounded-full bg-card text-sm capitalize">
      <span
        class="w-2.5 h-2.5 rounded-full {statusState === 'connected' ? 'bg-success' : statusState === 'error' ? 'bg-danger' : 'bg-muted-foreground animate-pulse'}"
      ></span>
      <span class="text-foreground">{statusState}</span>
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
</header>
