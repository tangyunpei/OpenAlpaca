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
  import ConversationsPanel from "$lib/components/ConversationsPanel.svelte";
  import ChatPanel from "$lib/components/ChatPanel.svelte";

  import { loadTasks, subscribeToTaskEvents } from "$lib/stores/tasks";
  import { loadAgents, subscribeToAgentEvents } from "$lib/stores/agents";
  import { loadSettings, subscribeToKeyEvents } from "$lib/stores/settings";
  import { subscribeToChatEvents } from "$lib/stores/chat";

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
    { id: "conversations", label: "Conversations" },
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
  let unsubChatEvents: (() => void) | null = null;

  onMount(() => {
    connectToDaemon();
    unsubTaskEvents = subscribeToTaskEvents();
    unsubAgentEvents = subscribeToAgentEvents();
    unsubKeyEvents = subscribeToKeyEvents();
    unsubChatEvents = subscribeToChatEvents();
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
    unsubChatEvents?.();
  });

  function handleTabChange(id: string) {
    activeTab = id;
    if (id === "connectors") connectorPanel?.refreshConnectors();
    if (id === "tasks") loadTasks();
    if (id === "agents") loadAgents();
    if (id === "settings") settingsPanel?.refreshSettings();
  }
</script>

<main class="w-full min-h-screen flex flex-col px-8 py-5 max-sm:px-3">
  <AppHeader {statusState} {info} />

  {#if error}
    <div class="mb-4 rounded-lg border border-danger bg-danger/20 px-4 py-3 text-sm text-danger">
      {error}
    </div>
  {/if}

  <div class="flex flex-1 min-h-0 max-[900px]:flex-col">
    <aside class="flex-1 min-w-[300px] border-r border-primary h-[calc(100vh-120px)] overflow-hidden max-[900px]:min-w-full max-[900px]:h-[300px] max-[900px]:border-r-0 max-[900px]:border-b max-[900px]:border-primary">
      <ChatPanel />
    </aside>

    <div class="flex-[0_1_900px] max-w-[900px] min-w-0 pl-6 max-[900px]:flex-auto max-[900px]:max-w-full max-[900px]:pl-0 max-[900px]:pt-4">
      <TabBar {activeTab} {tabs} onTabChange={handleTabChange} />

      <div>
        {#if activeTab === "events"}
          <EventLog events={eventList} />
        {:else if activeTab === "connectors"}
          <ConnectorPanel bind:this={connectorPanel} connectionState={statusState} />
        {:else if activeTab === "tasks"}
          <TaskPanel />
        {:else if activeTab === "agents"}
          <AgentPanel />
        {:else if activeTab === "conversations"}
          <ConversationsPanel />
        {:else if activeTab === "settings"}
          <SettingsPanel bind:this={settingsPanel} />
        {/if}
      </div>
    </div>
  </div>
</main>
