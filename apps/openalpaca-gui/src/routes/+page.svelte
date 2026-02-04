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
  import { shutdownDaemon } from "$lib/daemon_control";

  import {
    getConnectors,
    performConnectorAction,
    generateLinkToken,
    configureConnector,
    type Connector,
  } from "$lib/connectors";

  // Reactive state using Svelte 5 runes
  let statusState = $state("disconnected");
  let info = $state<{ baseUrl: string; instanceId: string } | null>(null);
  let eventList = $state<ServerEvent[]>([]);
  let error = $state<string | null>(null);

  // Tabs and Connectors state
  let activeTab = $state<"events" | "connectors">("events");
  let connectorsList = $state<Connector[]>([]);
  let linkToken = $state<string | null>(null);
  let isLoadingConnectors = $state(false);

  // Configuration Modal State
  let showConfigModal = $state(false);
  let configTargetId = $state<string | null>(null);
  let configToken = $state("");

  // Store subscriptions
  const unsubState = connectionState.subscribe((v) => {
    statusState = v;
    if (v === "connected" && activeTab === "connectors") {
      refreshConnectors();
    }
  });
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

  async function refreshConnectors() {
    if (statusState !== "connected") return;
    isLoadingConnectors = true;
    try {
      connectorsList = await getConnectors();
    } catch (e) {
      console.error("Failed to load connectors:", e);
    } finally {
      isLoadingConnectors = false;
    }
  }

  async function handleToggle(id: string, currentlyActive: boolean) {
    const action = currentlyActive ? "disable" : "enable";
    await handleAction(id, action);
  }

  async function handleAction(
    id: string,
    action: "enable" | "disable" | "delete",
  ) {
    try {
      await performConnectorAction(id, action);
      await refreshConnectors();
    } catch (e) {
      alert(`Action failed: ${e}`);
    }
  }

  function openConfigModal(id: string) {
    configTargetId = id;
    configToken = "";
    showConfigModal = true;
  }

  async function handleConfigSubmit() {
    if (!configTargetId || !configToken) return;
    try {
      await configureConnector(configTargetId, configToken);
      showConfigModal = false;
      await refreshConnectors();
    } catch (e) {
      alert(`Configuration failed: ${e}`);
    }
  }

  async function handleGenerateToken() {
    try {
      linkToken = await generateLinkToken();
    } catch (e) {
      alert(`Failed: ${e}`);
    }
  }

  function formatTime(ts: string): string {
    const date = new Date(ts);
    return date.toLocaleTimeString("en-US", {
      hour12: false,
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  }

  function getStatusLabel(status: string): string {
    switch (status) {
      case "unconfigured":
        return "Not Configured";
      case "active":
        return "Active";
      case "disabled":
        return "Disabled";
      case "error":
        return "Error";
      default:
        return status;
    }
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
    <div class="brand">
      <h1>🦙 OpenAlpaca</h1>
      {#if info}
        <div class="info-tag">
          <span class="instance">{info.instanceId.slice(0, 8)}</span>
          <span class="url">{info.baseUrl}</span>
        </div>
      {/if}
    </div>
    <div
      class="status"
      class:connected={statusState === "connected"}
      class:error={statusState === "error"}
    >
      <span class="dot"></span>
      <span class="text">{statusState}</span>
    </div>
  </header>

  {#if error}
    <div class="error-banner">{error}</div>
  {/if}

  <div class="nav-tabs">
    <button
      class="tab-btn"
      class:active={activeTab === "events"}
      onclick={() => (activeTab = "events")}
    >
      Event Log
    </button>
    <button
      class="tab-btn"
      class:active={activeTab === "connectors"}
      onclick={() => {
        activeTab = "connectors";
        refreshConnectors();
      }}
    >
      Connectors
    </button>
  </div>

  <div class="tab-content">
    {#if activeTab === "events"}
      <div class="controls">
        <button
          onclick={() => connectToDaemon()}
          disabled={statusState === "connecting"}
        >
          {statusState === "connecting" ? "Connecting..." : "Reconnect"}
        </button>
        <button onclick={() => clearEvents()}>Clear</button>
        <button class="danger" onclick={() => shutdownDaemon()}
          >Quit OpenAlpaca</button
        >
      </div>

      <div class="view-panel">
        <div class="panel-header">
          <h2>Events ({eventList.length})</h2>
        </div>
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
    {:else if activeTab === "connectors"}
      <div class="controls">
        <button onclick={refreshConnectors} disabled={isLoadingConnectors}>
          {isLoadingConnectors ? "Refreshing..." : "Refresh"}
        </button>
        <button onclick={handleGenerateToken}>Generate Bind Token</button>
      </div>

      {#if linkToken}
        <div class="token-panel">
          <p>Binding Code (expires in 5m):</p>
          <div class="token-box">
            <code>{linkToken}</code>
          </div>
          <p class="hint">
            Send <code>/link {linkToken}</code> to your Telegram bot.
          </p>
        </div>
      {/if}

      <div class="view-panel">
        <div class="panel-header">
          <h2>Platform Connectors</h2>
        </div>
        <div class="connector-list">
          {#each connectorsList as connector}
            <div class="connector-card">
              <div class="connector-info">
                <h3>{connector.name}</h3>
                <span class="status-badge {connector.status}">
                  {getStatusLabel(connector.status)}
                </span>
              </div>
              <div class="connector-actions">
                <button
                  class="action-btn icon"
                  title="Configure"
                  onclick={() => openConfigModal(connector.id)}
                >
                  ⚙️
                </button>

                <label class="switch">
                  <input
                    type="checkbox"
                    checked={connector.status === "active"}
                    onchange={() =>
                      handleToggle(connector.id, connector.status === "active")}
                  />
                  <span class="slider"></span>
                </label>
                <button
                  class="action-btn danger icon"
                  title="Clear Config"
                  onclick={() => handleAction(connector.id, "delete")}
                >
                  ✕
                </button>
              </div>
            </div>
          {:else}
            <div class="empty">
              No connectors found. Check your configuration.
            </div>
          {/each}
        </div>
      </div>
    {/if}
  </div>

  {#if showConfigModal}
    <div class="modal-backdrop">
      <div class="modal">
        <h3>Configure {configTargetId}</h3>
        <p>Enter the authentication token for this connector.</p>
        <input
          type="text"
          bind:value={configToken}
          placeholder="Token (e.g. 12345:ABC...)"
          class="token-input"
        />
        <div class="modal-actions">
          <button class="secondary" onclick={() => (showConfigModal = false)}
            >Cancel</button
          >
          <button onclick={handleConfigSubmit}>Save & Enable</button>
        </div>
      </div>
    </div>
  {/if}
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
    font-family:
      "Inter",
      -apple-system,
      BlinkMacSystemFont,
      sans-serif;
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
    margin-bottom: 25px;
    padding-bottom: 15px;
    border-bottom: 2px solid var(--primary);
  }

  .brand {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  h1 {
    margin: 0;
    font-size: 2rem;
    font-weight: 800;
    color: var(--text);
    letter-spacing: -0.5px;
  }

  .info-tag {
    display: flex;
    gap: 12px;
    font-family: "Fira Code", monospace;
    font-size: 0.75rem;
    color: var(--text-dim);
  }

  .info-tag .instance {
    background: var(--primary);
    padding: 0 6px;
    border-radius: 4px;
    color: var(--text);
  }

  .nav-tabs {
    display: flex;
    gap: 8px;
    margin-bottom: 20px;
    background: var(--surface);
    padding: 6px;
    border-radius: 10px;
  }

  .tab-btn {
    flex: 1;
    padding: 10px;
    border: none;
    background: transparent;
    color: var(--text-dim);
    font-weight: 600;
    border-radius: 6px;
    transition: all 0.2s;
  }

  .tab-btn:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--text);
  }

  .tab-btn.active {
    background: var(--primary);
    color: #fff;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.3);
  }

  .view-panel {
    background: var(--surface);
    border-radius: 12px;
    padding: 0;
    overflow: hidden;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.2);
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

  .connector-list {
    padding: 10px;
  }

  .connector-card {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 16px 20px;
    background: rgba(255, 255, 255, 0.03);
    border-radius: 10px;
    margin-bottom: 10px;
    border: 1px solid rgba(255, 255, 255, 0.05);
    transition: all 0.2s;
  }

  .connector-card:hover {
    background: rgba(255, 255, 255, 0.06);
    transform: translateX(4px);
    border-color: var(--primary);
  }

  .connector-info h3 {
    margin: 0 0 4px 0;
    font-size: 1.1rem;
  }

  .status-badge {
    font-size: 0.75rem;
    padding: 2px 8px;
    border-radius: 20px;
    text-transform: uppercase;
    font-weight: 700;
  }

  .status-badge.active {
    background: rgba(16, 185, 129, 0.2);
    color: var(--success);
  }
  .status-badge.error {
    background: rgba(239, 68, 68, 0.2);
    color: var(--error);
  }
  .status-badge.disabled {
    background: rgba(107, 114, 128, 0.2);
    color: #9ca3af;
  }
  .status-badge.unconfigured {
    background: rgba(255, 255, 255, 0.05);
    color: var(--text-dim);
  }

  .connector-actions {
    display: flex;
    align-items: center;
    gap: 16px;
  }

  .action-btn {
    padding: 8px 14px;
    font-size: 0.8rem;
    border-radius: 6px;
    cursor: pointer;
    border: none;
    transition: all 0.2s;
  }

  .action-btn.icon {
    padding: 6px 10px;
    font-size: 1.2rem;
    font-weight: bold;
    line-height: 1;
  }

  .action-btn.danger {
    background: rgba(239, 68, 68, 0.1);
    color: var(--error);
  }
  .action-btn.danger:hover {
    background: var(--error);
    color: white;
  }

  /* Toggle Switch */
  .switch {
    position: relative;
    display: inline-block;
    width: 44px;
    height: 24px;
    flex-shrink: 0;
  }

  .switch input {
    opacity: 0;
    width: 0;
    height: 0;
  }

  .slider {
    position: absolute;
    cursor: pointer;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background-color: var(--primary);
    transition: 0.4s;
    border-radius: 24px;
  }

  .slider:before {
    position: absolute;
    content: "";
    height: 18px;
    width: 18px;
    left: 3px;
    bottom: 3px;
    background-color: white;
    transition: 0.4s;
    border-radius: 50%;
  }

  input:checked + .slider {
    background-color: var(--success);
  }

  input:checked + .slider:before {
    transform: translateX(20px);
  }

  .token-panel {
    background: linear-gradient(135deg, var(--primary), #1a3a5f);
    padding: 20px;
    border-radius: 12px;
    margin-bottom: 20px;
    text-align: center;
    border: 1px solid var(--accent);
  }

  .token-box {
    background: rgba(0, 0, 0, 0.3);
    padding: 15px;
    border-radius: 8px;
    font-size: 2rem;
    letter-spacing: 4px;
    margin: 10px 0;
    color: var(--accent);
    font-weight: 800;
  }

  .hint {
    font-size: 0.85rem;
    color: var(--text-dim);
  }

  .controls {
    display: flex;
    gap: 10px;
    margin-bottom: 20px;
  }

  button:not(.action-btn) {
    padding: 10px 20px;
    border: none;
    border-radius: 8px;
    background: var(--primary);
    color: var(--text);
    font-size: 0.9rem;
    cursor: pointer;
    transition: all 0.2s;
  }

  button:not(.action-btn):hover:not(:disabled) {
    background: var(--accent);
    transform: translateY(-1px);
  }

  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  button.danger:not(.action-btn) {
    background: rgba(239, 68, 68, 0.2);
    border: 1px solid var(--error);
    color: var(--error);
  }

  button.danger:not(.action-btn):hover:not(:disabled) {
    background: var(--error);
    color: #fff;
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
    font-size: 1.2rem;
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

  .event .message,
  .event .command {
    color: var(--text-dim);
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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

  .empty {
    color: var(--text-dim);
    text-align: center;
    padding: 60px 40px;
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

  /* Modal Styling */
  .modal-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    width: 100%;
    height: 100%;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    justify-content: center;
    align-items: center;
    z-index: 1000;
  }

  .modal {
    background: var(--surface);
    padding: 24px;
    border-radius: 12px;
    width: 90%;
    max-width: 400px;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);
    border: 1px solid var(--border);
  }

  .modal h3 {
    margin-top: 0;
    color: var(--primary);
  }

  .token-input {
    width: 100%;
    padding: 10px;
    margin: 15px 0;
    background: var(--bg-dark);
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 6px;
    box-sizing: border-box;
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }

  .action-btn.icon {
    background: transparent;
    border: 1px solid var(--border);
    padding: 6px;
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
  }

  .action-btn.icon:hover {
    background: var(--primary);
    border-color: var(--primary);
  }
</style>
