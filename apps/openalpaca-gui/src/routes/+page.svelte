<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    connectToDaemon,
    disconnect,
    clearEvents,
    connectionState,
    connectionInfo,
    events,
    errorMessage,
    type ServerEvent,
  } from "$lib/daemon";

  // Subscribe to stores using regular variables
  let state: string = "disconnected";
  let info: { base_url: string; instance_id: string } | null = null;
  let eventList: ServerEvent[] = [];
  let error: string | null = null;

  // Store subscriptions
  const unsubState = connectionState.subscribe((v) => (state = v));
  const unsubInfo = connectionInfo.subscribe((v) => (info = v));
  const unsubEvents = events.subscribe((v) => (eventList = v));
  const unsubError = errorMessage.subscribe((v) => (error = v));

  onMount(() => {
    connectToDaemon();
  });

  onDestroy(() => {
    disconnect();
    unsubState();
    unsubInfo();
    unsubEvents();
    unsubError();
  });

  function formatTime(ts: string): string {
    const date = new Date(ts);
    return date.toLocaleTimeString("en-US", {
      hour12: false,
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  }

  function getEventIcon(type: string): string {
    switch (type) {
      case "heartbeat":
        return "💓";
      case "log":
        return "📝";
      case "command_received":
        return "⚡";
      case "agent_status":
        return "🤖";
      case "task_update":
        return "📋";
      default:
        return "📨";
    }
  }
</script>

<main class="container">
  <header class="header">
    <h1>🦙 OpenAlpaca</h1>
    <div class="status" class:connected={state === "connected"} class:error={state === "error"}>
      <span class="dot"></span>
      <span class="text">{state}</span>
    </div>
  </header>

  {#if error}
    <div class="error-banner">{error}</div>
  {/if}

  {#if info}
    <div class="info-bar">
      <span class="instance">Instance: {info.instance_id.slice(0, 8)}...</span>
      <span class="url">{info.base_url}</span>
    </div>
  {/if}

  <div class="controls">
    <button onclick={() => connectToDaemon()} disabled={state === "connecting"}>
      {state === "connecting" ? "Connecting..." : "Reconnect"}
    </button>
    <button onclick={() => clearEvents()}>Clear</button>
  </div>

  <div class="event-log">
    <h2>Events ({eventList.length})</h2>
    <ul class="events">
      {#each eventList as event (event.ts)}
        <li class="event {event.type}">
          <span class="icon">{getEventIcon(event.type)}</span>
          <span class="time">{formatTime(event.ts)}</span>
          <span class="type">{event.type}</span>
          {#if event.message}
            <span class="message">{event.message}</span>
          {/if}
          {#if event.command}
            <span class="command">cmd: {event.command}</span>
          {/if}
        </li>
      {:else}
        <li class="empty">No events yet...</li>
      {/each}
    </ul>
  </div>
</main>

<style>
  :root {
    --bg: #1a1a2e;
    --surface: #16213e;
    --primary: #0f3460;
    --accent: #e94560;
    --text: #eaeaea;
    --text-dim: #8892b0;
    --success: #10b981;
    --error: #ef4444;
  }

  :global(body) {
    margin: 0;
    padding: 0;
    background: var(--bg);
    color: var(--text);
    font-family: "Inter", -apple-system, BlinkMacSystemFont, sans-serif;
    min-height: 100vh;
  }

  .container {
    max-width: 900px;
    margin: 0 auto;
    padding: 20px;
  }

  .header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 20px;
    padding-bottom: 15px;
    border-bottom: 1px solid var(--primary);
  }

  h1 {
    margin: 0;
    font-size: 1.8rem;
    background: linear-gradient(135deg, var(--accent), #ff6b6b);
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    background-clip: text;
  }

  .status {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    border-radius: 20px;
    background: var(--surface);
    font-size: 0.85rem;
    text-transform: capitalize;
  }

  .status .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--text-dim);
    animation: pulse 2s infinite;
  }

  .status.connected .dot {
    background: var(--success);
  }

  .status.error .dot {
    background: var(--error);
    animation: none;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.5;
    }
  }

  .error-banner {
    background: rgba(239, 68, 68, 0.2);
    border: 1px solid var(--error);
    color: var(--error);
    padding: 12px 16px;
    border-radius: 8px;
    margin-bottom: 15px;
  }

  .info-bar {
    display: flex;
    justify-content: space-between;
    background: var(--surface);
    padding: 10px 16px;
    border-radius: 8px;
    margin-bottom: 15px;
    font-size: 0.85rem;
    color: var(--text-dim);
  }

  .controls {
    display: flex;
    gap: 10px;
    margin-bottom: 20px;
  }

  button {
    padding: 10px 20px;
    border: none;
    border-radius: 8px;
    background: var(--primary);
    color: var(--text);
    font-size: 0.9rem;
    cursor: pointer;
    transition: all 0.2s;
  }

  button:hover:not(:disabled) {
    background: var(--accent);
    transform: translateY(-1px);
  }

  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .event-log {
    background: var(--surface);
    border-radius: 12px;
    padding: 20px;
  }

  .event-log h2 {
    margin: 0 0 15px 0;
    font-size: 1.1rem;
    font-weight: 500;
  }

  .events {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 60vh;
    overflow-y: auto;
  }

  .event {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border-radius: 6px;
    margin-bottom: 6px;
    background: rgba(255, 255, 255, 0.03);
    font-size: 0.9rem;
    transition: background 0.15s;
  }

  .event:hover {
    background: rgba(255, 255, 255, 0.06);
  }

  .event .icon {
    font-size: 1.1rem;
  }

  .event .time {
    color: var(--text-dim);
    font-family: "Fira Code", monospace;
    font-size: 0.8rem;
  }

  .event .type {
    background: var(--primary);
    padding: 2px 8px;
    border-radius: 4px;
    font-size: 0.75rem;
    text-transform: uppercase;
  }

  .event.heartbeat .type {
    background: rgba(16, 185, 129, 0.2);
    color: var(--success);
  }

  .event.log .type {
    background: rgba(59, 130, 246, 0.2);
    color: #60a5fa;
  }

  .event.command_received .type {
    background: rgba(233, 69, 96, 0.2);
    color: var(--accent);
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
    padding: 40px;
  }

  /* Scrollbar styling */
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
</style>
