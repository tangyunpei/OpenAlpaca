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
  import TaskPanel from "$lib/components/TaskPanel.svelte";
  import AgentStatusPanel from "$lib/components/AgentStatusPanel.svelte";
  import ChatPanel from "$lib/components/ChatPanel.svelte";
  import SettingsDrawer from "$lib/components/SettingsDrawer.svelte";

  import { loadTasks, subscribeToTaskEvents, activeTasks } from "$lib/stores/tasks";
  import { loadAgents, subscribeToAgentEvents } from "$lib/stores/agents";
  import { loadTemplates } from "$lib/stores/templates";
  import { loadInstances, subscribeToInstanceEvents, instanceList } from "$lib/stores/instances";
  import { subscribeToKeyEvents } from "$lib/stores/settings";
  import { subscribeToChatEvents } from "$lib/stores/chat";

  // Reactive state from stores
  let statusState = $state("disconnected");
  let info = $state<{ baseUrl: string; instanceId: string } | null>(null);
  let eventList = $state<ServerEvent[]>([]);
  let error = $state<string | null>(null);

  let rightTab = $state<"tasks" | "agents">("tasks");
  let drawerOpen = $state(false);

  // Counts for tab badges
  let activeTaskCount = $state(0);
  let instanceCount = $state(0);
  const unsubActiveCount = activeTasks.subscribe((v) => (activeTaskCount = v.length));
  const unsubInstanceCount = instanceList.subscribe((v) => (instanceCount = v.length));

  // Store subscriptions
  const unsubState = connectionState.subscribe((v) => {
    statusState = v;
    if (v === "connected") {
      loadTasks();
      loadAgents();
      loadTemplates();
      loadInstances();
    }
  });
  const unsubInfo = connectionInfo.subscribe((v) => (info = v));
  const unsubEvents = events.subscribe((v) => (eventList = v));
  const unsubError = errorMessage.subscribe((v) => (error = v));

  let unsubTaskEvents: (() => void) | null = null;
  let unsubAgentEvents: (() => void) | null = null;
  let unsubInstanceEvents: (() => void) | null = null;
  let unsubKeyEvents: (() => void) | null = null;
  let unsubChatEvents: (() => void) | null = null;

  onMount(() => {
    connectToDaemon();
    unsubTaskEvents = subscribeToTaskEvents();
    unsubAgentEvents = subscribeToAgentEvents();
    unsubInstanceEvents = subscribeToInstanceEvents();
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
    unsubInstanceEvents?.();
    unsubKeyEvents?.();
    unsubChatEvents?.();
    unsubActiveCount();
    unsubInstanceCount();
  });

  function toggleDrawer() {
    drawerOpen = !drawerOpen;
  }

  function handleRightTabChange(tab: "tasks" | "agents") {
    rightTab = tab;
    if (tab === "tasks") loadTasks();
    if (tab === "agents") {
      loadTemplates();
      loadInstances();
    }
  }
</script>

<main class="w-full min-h-screen flex flex-col px-8 py-5 max-sm:px-3">
  <AppHeader {statusState} {info} onToggleSettings={toggleDrawer} />

  {#if error}
    <div class="mb-4 rounded-lg border border-danger bg-danger/20 px-4 py-3 text-sm text-danger flex items-center justify-between gap-3 animate-fadeIn">
      <span class="flex-1">{error}</span>
      <button
        class="shrink-0 w-6 h-6 flex items-center justify-center rounded bg-transparent text-danger hover:bg-danger/30 transition-colors cursor-pointer border-none text-base leading-none"
        onclick={() => (error = null)}
        aria-label="Dismiss error"
      >&times;</button>
    </div>
  {/if}

  <div class="flex flex-1 min-h-0 max-[900px]:flex-col">
    <aside class="flex-1 min-w-[300px] border-r border-primary h-[calc(100vh-120px)] overflow-hidden max-[900px]:min-w-full max-[900px]:h-[50vh] max-[900px]:border-r-0 max-[900px]:border-b max-[900px]:border-primary">
      <ChatPanel />
    </aside>

    <div class="flex-[0_1_900px] max-w-[900px] min-w-0 pl-6 max-[900px]:flex-auto max-[900px]:max-w-full max-[900px]:pl-0 max-[900px]:pt-4">
      <!-- Tasks / Agents switcher -->
      <div class="flex bg-black/25 p-1 rounded-[10px] mx-auto mb-6 gap-1 w-fit border border-white/5 backdrop-blur-[10px]">
        <button
          class="px-6 py-2 border-none bg-transparent text-muted-foreground cursor-pointer text-sm font-medium rounded-md transition-all duration-200 whitespace-nowrap hover:text-foreground {rightTab === 'tasks' ? 'bg-white/10 text-white shadow-sm' : ''}"
          onclick={() => handleRightTabChange('tasks')}
        >
          Tasks
          {#if activeTaskCount > 0}
            <span class="ml-1.5 text-[0.65rem] px-1.5 py-px rounded-full bg-accent/20 text-accent font-bold">{activeTaskCount}</span>
          {/if}
        </button>
        <button
          class="px-6 py-2 border-none bg-transparent text-muted-foreground cursor-pointer text-sm font-medium rounded-md transition-all duration-200 whitespace-nowrap hover:text-foreground {rightTab === 'agents' ? 'bg-white/10 text-white shadow-sm' : ''}"
          onclick={() => handleRightTabChange('agents')}
        >
          Agents
          {#if instanceCount > 0}
            <span class="ml-1.5 text-[0.65rem] px-1.5 py-px rounded-full bg-accent/20 text-accent font-bold">{instanceCount}</span>
          {/if}
        </button>
      </div>

      <div>
        {#if rightTab === "tasks"}
          <TaskPanel />
        {:else}
          <AgentStatusPanel />
        {/if}
      </div>
    </div>
  </div>

  <SettingsDrawer
    open={drawerOpen}
    onClose={() => drawerOpen = false}
    {eventList}
    connectionState={statusState}
  />
</main>
