<script lang="ts">
  import { connectToDaemon } from "$lib/daemon";
  import { Button } from "$lib/ui";

  interface Props {
    statusState: string;
    info: { baseUrl: string; instanceId: string } | null;
  }

  let { statusState, info }: Props = $props();
</script>

<header class="flex justify-between items-center mb-6 pb-4 border-b-2 border-primary flex-wrap gap-2.5">
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
