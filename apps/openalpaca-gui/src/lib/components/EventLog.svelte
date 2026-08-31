<script lang="ts">
  import { clearEvents, type ServerEvent } from "$lib/daemon";
  import { shutdownDaemon } from "$lib/daemon_control";
  import { formatTime } from "$lib/utils";

  interface Props {
    events: ServerEvent[];
  }

  let { events }: Props = $props();

  function getEventIcon(type: string): string {
    switch (type) {
      case "heartbeat":
        return `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z"/></svg>`;
      case "log":
        return `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><line x1="10" y1="9" x2="8" y2="9"/></svg>`;
      case "command_received":
        return `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="m5 14 7-9 1 11h6L12 25l-1-11H5Z"/></svg>`;
      case "agent_status":
        return `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/></svg>`;
      case "task_status":
        return `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect width="16" height="20" x="4" y="2" rx="2"/><line x1="8" y1="6" x2="16" y2="6"/><line x1="8" y1="10" x2="16" y2="10"/><line x1="8" y1="14" x2="16" y2="14"/><line x1="8" y1="18" x2="16" y2="18"/></svg>`;
      case "connector_status":
        return `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2v4m0 12v4M4.93 4.93l2.83 2.83m8.48 8.48l2.83 2.83M2 12h4m12 0h4M4.93 19.07l2.83-2.83m8.48-8.48l2.83-2.83"/></svg>`;
      default:
        return `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M22 17a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V9.5C2 7 4 5 6.5 5H18c2.2 0 4 1.8 4 4v8Z"/><path d="m22 9-10 7L2 9"/></svg>`;
    }
  }

  const typeColors: Record<string, string> = {
    heartbeat: "bg-success/10 text-success",
    log: "bg-blue-400/10 text-blue-400",
    command_received: "bg-accent/10 text-accent",
    wake: "bg-violet-500/10 text-violet-400",
    task_status: "bg-amber-400/10 text-amber-400",
    agent_status: "bg-blue-400/10 text-blue-400",
    connector_status: "bg-violet-500/10 text-violet-400",
    workflow_started: "bg-success/10 text-success",
    workflow_steered: "bg-blue-400/10 text-blue-400",
    workflow_progress: "bg-amber-400/10 text-amber-400",
    followup_queued: "bg-violet-500/10 text-violet-400",
  };
</script>

<div class="flex flex-wrap gap-2.5 mb-5 max-[480px]:flex-wrap">
  <button
    class="inline-flex items-center justify-center px-5 py-2 text-sm font-medium rounded-lg bg-primary text-foreground hover:bg-accent hover:text-accent-foreground transition-all cursor-pointer"
    onclick={() => clearEvents()}
  >Clear</button>
  <button
    class="inline-flex items-center justify-center px-5 py-2 text-sm font-medium rounded-lg bg-danger/20 border border-danger text-danger hover:bg-danger hover:text-white transition-all cursor-pointer"
    onclick={() => shutdownDaemon()}
  >Quit OpenAlpaca</button>
</div>

<div class="oa-panel">
  <div class="oa-panel-header">
    <h2>Events ({events.length})</h2>
  </div>
  <ul class="list-none m-0 p-2.5 max-h-[60vh] overflow-y-auto">
    {#each events as event (event._id)}
      <li class="flex items-center gap-3 px-4 py-3 rounded-lg mb-1.5 bg-white/2 text-sm transition-colors hover:bg-white/4 max-[480px]:gap-2 max-[480px]:px-3 max-[480px]:py-2.5 max-[480px]:text-xs">
        <span class="flex items-center justify-center w-6 h-6 shrink-0 max-[480px]:hidden">
          {@html getEventIcon(event.type)}
        </span>
        <span class="text-muted-foreground font-mono text-xs">{"ts" in event ? formatTime(event.ts) : ""}</span>
        <span class="px-2 py-0.5 rounded text-[0.75rem] uppercase font-bold min-w-[80px] text-center max-[480px]:min-w-[60px] max-[480px]:text-[0.65rem] {typeColors[event.type] || 'bg-white/5 text-muted-foreground'}">
          {event.type}
        </span>
        {#if event.type === "log"}
          <span class="text-muted-foreground flex-1 overflow-hidden text-ellipsis whitespace-nowrap">{event.message}</span>
        {/if}
        {#if event.type === "command_received"}
          <span class="text-muted-foreground flex-1 overflow-hidden text-ellipsis whitespace-nowrap">cmd: {event.command}</span>
        {/if}
        {#if event.type === "workflow_started"}
          <span class="text-muted-foreground flex-1 overflow-hidden text-ellipsis whitespace-nowrap">{event.title}</span>
        {/if}
        {#if event.type === "workflow_progress"}
          <span class="text-muted-foreground flex-1 overflow-hidden text-ellipsis whitespace-nowrap">{event.message}</span>
        {/if}
      </li>
    {:else}
      <li class="text-muted-foreground text-center py-15 px-10">No events yet...</li>
    {/each}
  </ul>
</div>
