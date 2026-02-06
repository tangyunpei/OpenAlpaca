<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    connectToDaemon,
    disconnect,
    connectionState,
    connectionInfo,
    events,
    errorMessage,
    type ServerEvent,
  } from "$lib/daemon";

  import AppHeader from "$lib/components/AppHeader.svelte";
  import TabBar from "$lib/components/TabBar.svelte";
  import EventLog from "$lib/components/EventLog.svelte";
  import ConnectorPanel from "$lib/components/ConnectorPanel.svelte";
  import TaskPanel from "$lib/components/TaskPanel.svelte";
  import AgentPanel from "$lib/components/AgentPanel.svelte";
  import SettingsPanel from "$lib/components/SettingsPanel.svelte";

  import { loadTasks, subscribeToTaskEvents } from "$lib/stores/tasks";
  import { loadAgents, subscribeToAgentEvents } from "$lib/stores/agents";
  import { loadSettings, subscribeToKeyEvents } from "$lib/stores/settings";

  // Reactive state from stores
  let statusState = $state("disconnected");
  let info = $state<{ baseUrl: string; instanceId: string } | null>(null);
  let eventList = $state<ServerEvent[]>([]);
  let error = $state<string | null>(null);

  let activeTab = $state<string>("events");

  let connectorPanel: ConnectorPanel | undefined = $state();
  let settingsPanel: SettingsPanel | undefined = $state();

  const tabs = [
    { id: "events", label: "Event Log" },
    { id: "connectors", label: "Connectors" },
    { id: "tasks", label: "Tasks" },
    { id: "agents", label: "Agents" },
    { id: "settings", label: "Settings" },
  ];

  // Store subscriptions
  const unsubState = connectionState.subscribe((v) => {
    statusState = v;
    if (v === "connected") {
      if (activeTab === "connectors") connectorPanel?.refreshConnectors();
      loadTasks();
      loadAgents();
    }
  });
  const unsubInfo = connectionInfo.subscribe((v) => (info = v));
  const unsubEvents = events.subscribe((v) => (eventList = v));
  const unsubError = errorMessage.subscribe((v) => (error = v));

  let unsubTaskEvents: (() => void) | null = null;
  let unsubAgentEvents: (() => void) | null = null;
  let unsubKeyEvents: (() => void) | null = null;

  onMount(() => {
    connectToDaemon();
    unsubTaskEvents = subscribeToTaskEvents();
    unsubAgentEvents = subscribeToAgentEvents();
    unsubKeyEvents = subscribeToKeyEvents();
  });

  onDestroy(() => {
    disconnect();
    unsubState();
    unsubInfo();
    unsubEvents();
    unsubError();
    unsubTaskEvents?.();
    unsubAgentEvents?.();
    unsubKeyEvents?.();
  });

  function handleTabChange(id: string) {
    activeTab = id;
    if (id === "connectors") connectorPanel?.refreshConnectors();
    if (id === "tasks") loadTasks();
    if (id === "agents") loadAgents();
    if (id === "settings") settingsPanel?.refreshSettings();
  }
</script>

<main class="container">
  <AppHeader {statusState} {info} />

  {#if error}
    <div class="error-banner">{error}</div>
  {/if}

  <TabBar {activeTab} {tabs} onTabChange={handleTabChange} />

  <div class="tab-content">
    {#if activeTab === "events"}
      <EventLog events={eventList} connectionState={statusState} />
    {:else if activeTab === "connectors"}
      <ConnectorPanel bind:this={connectorPanel} connectionState={statusState} />
    {:else if activeTab === "tasks"}
      <TaskPanel />
    {:else if activeTab === "agents"}
      <AgentPanel />
    {:else if activeTab === "settings"}
      <SettingsPanel bind:this={settingsPanel} />
    {/if}
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
    font-family:
      "Inter",
      -apple-system,
      BlinkMacSystemFont,
      sans-serif;
    min-height: 100vh;
  }

  .container {
    padding: 20px 32px;
    width: 100%;
    box-sizing: border-box;
    min-height: 100vh;
  }

  .error-banner {
    background: rgba(239, 68, 68, 0.2);
    border: 1px solid var(--error);
    color: var(--error);
    padding: 12px 16px;
    border-radius: 8px;
    margin-bottom: 15px;
  }

  /* Global button base styles */
  :global(button:not(.action-btn):not(.tab-btn):not(.filter-btn):not(.close-btn)) {
    padding: 10px 20px;
    border: none;
    border-radius: 8px;
    background: var(--primary);
    color: var(--text);
    font-size: 0.9rem;
    cursor: pointer;
    transition: all 0.2s;
  }

  :global(button:not(.action-btn):not(.tab-btn):not(.filter-btn):not(.close-btn):hover:not(:disabled)) {
    background: var(--accent);
    transform: translateY(-1px);
  }

  :global(button:disabled) {
    opacity: 0.5;
    cursor: not-allowed;
  }

  :global(button.danger:not(.action-btn)) {
    background: rgba(239, 68, 68, 0.2);
    border: 1px solid var(--error);
    color: var(--error);
  }

  :global(button.danger:not(.action-btn):hover:not(:disabled)) {
    background: var(--error);
    color: #fff;
  }

  :global(button.secondary) {
    background: rgba(255, 255, 255, 0.05);
    color: var(--text-dim);
  }

  /* Responsive layout */
  @media (max-width: 640px) {
    .container {
      padding: 12px;
    }
  }
</style>
