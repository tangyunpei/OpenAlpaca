<script lang="ts">
  import { connectToDaemon, clearEvents, type ServerEvent } from "$lib/daemon";
  import { shutdownDaemon } from "$lib/daemon_control";
  import { formatTime } from "$lib/utils";

  interface Props {
    events: ServerEvent[];
    connectionState: string;
  }

  let { events, connectionState }: Props = $props();

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
</script>

<div class="controls">
  <button
    onclick={() => connectToDaemon()}
    disabled={connectionState === "connecting"}
  >
    {connectionState === "connecting" ? "Connecting..." : "Reconnect"}
  </button>
  <button onclick={() => clearEvents()}>Clear</button>
  <button class="danger" onclick={() => shutdownDaemon()}>Quit OpenAlpaca</button>
</div>

<div class="view-panel">
  <div class="panel-header">
    <h2>Events ({events.length})</h2>
  </div>
  <ul class="events">
    {#each events as event (event.ts)}
      <li class="event {event.type}">
        <span class="icon">{@html getEventIcon(event.type)}</span>
        <span class="time">{formatTime(event.ts)}</span>
        <span class="type">{event.type}</span>
        {#if event.type === "log"}
          <span class="message">{event.message}</span>
        {/if}
        {#if event.type === "command_received"}
          <span class="command">cmd: {event.command}</span>
        {/if}
      </li>
    {:else}
      <li class="empty">No events yet...</li>
    {/each}
  </ul>
</div>

<style>
  .controls {
    display: flex;
    gap: 10px;
    margin-bottom: 20px;
  }

  .view-panel {
    background: rgba(30, 30, 50, 0.7);
    backdrop-filter: blur(20px);
    -webkit-backdrop-filter: blur(20px);
    border-radius: 16px;
    padding: 0;
    overflow: hidden;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }

  .panel-header {
    padding: 15px 20px;
    background: rgba(255, 255, 255, 0.02);
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
  }

  .view-panel h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
    color: var(--text);
  }

  .events {
    list-style: none;
    margin: 0;
    padding: 10px;
    max-height: 60vh;
    overflow-y: auto;
  }

  .event {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    border-radius: 8px;
    margin-bottom: 6px;
    background: rgba(255, 255, 255, 0.02);
    font-size: 0.9rem;
    transition: background 0.15s;
  }

  .event:hover {
    background: rgba(255, 255, 255, 0.04);
  }

  .event .icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    flex-shrink: 0;
  }

  .event .time {
    color: var(--text-dim);
    font-family: "Fira Code", monospace;
    font-size: 0.8rem;
  }

  .event .type {
    padding: 2px 8px;
    border-radius: 4px;
    font-size: 0.75rem;
    text-transform: uppercase;
    font-weight: 700;
    min-width: 80px;
    text-align: center;
  }

  .event.heartbeat .type {
    background: rgba(16, 185, 129, 0.1);
    color: var(--success);
  }

  .event.log .type {
    background: rgba(59, 130, 246, 0.1);
    color: #60a5fa;
  }

  .event.command_received .type {
    background: rgba(233, 69, 96, 0.1);
    color: var(--accent);
  }

  .event.wake .type {
    background: rgba(139, 92, 246, 0.1);
    color: #a78bfa;
  }

  .event.task_status .type {
    background: rgba(251, 191, 36, 0.1);
    color: #fbbf24;
  }

  .event.agent_status .type {
    background: rgba(96, 165, 250, 0.1);
    color: #60a5fa;
  }

  .event.connector_status .type {
    background: rgba(167, 139, 250, 0.1);
    color: #a78bfa;
  }

  .event .message,
  .event .command {
    color: var(--text-dim);
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .empty {
    color: var(--text-dim);
    text-align: center;
    padding: 60px 40px;
  }

  .events::-webkit-scrollbar {
    width: 6px;
  }

  .events::-webkit-scrollbar-track {
    background: transparent;
  }

  .events::-webkit-scrollbar-thumb {
    background: var(--primary);
    border-radius: 3px;
  }

  @media (max-width: 480px) {
    .controls {
      flex-wrap: wrap;
    }

    .event {
      gap: 8px;
      padding: 10px 12px;
      font-size: 0.8rem;
    }

    .event .type {
      min-width: 60px;
      font-size: 0.65rem;
    }

    .event .icon {
      display: none;
    }
  }
</style>
