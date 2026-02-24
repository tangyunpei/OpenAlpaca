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

<main class="w-full h-screen flex flex-col px-7 py-5 max-sm:px-3 overflow-hidden">
  <AppHeader {statusState} {info} onToggleSettings={toggleDrawer} />

  {#if error}
    <div class="mb-4 rounded-xl border border-danger/40 px-4 py-3 text-sm flex items-center justify-between gap-3 animate-slideDown"
         style="background: linear-gradient(135deg, hsl(2 68% 56% / 0.12) 0%, hsl(2 68% 56% / 0.06) 100%);">
      <span class="flex items-center gap-2 flex-1 text-danger">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="shrink-0">
          <circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/>
        </svg>
        {error}
      </span>
      <button
        class="shrink-0 w-6 h-6 flex items-center justify-center rounded-md bg-transparent text-danger/70 hover:bg-danger/20 hover:text-danger transition-colors cursor-pointer border-none"
        onclick={() => (error = null)}
        aria-label="Dismiss error"
      >&times;</button>
    </div>
  {/if}

  <div class="flex flex-1 min-h-0 max-[900px]:flex-col overflow-hidden gap-0">
    <!-- Chat panel (left) -->
    <aside class="flex-1 min-w-[320px] min-h-0 max-[900px]:min-w-full max-[900px]:flex-[0_0_50%]">
      <ChatPanel />
    </aside>

    <!-- Separator -->
    <div class="w-px self-stretch mx-4 max-[900px]:w-auto max-[900px]:h-px max-[900px]:mx-0 max-[900px]:my-3"
         style="background: linear-gradient(180deg, transparent, var(--color-border-strong) 20%, var(--color-border-strong) 80%, transparent);"></div>

    <!-- Right panel (tasks / agents) -->
    <div class="flex-1 max-w-[50%] min-w-0 overflow-y-auto min-h-0 max-[900px]:flex-auto max-[900px]:max-w-full">
      <!-- Tab switcher -->
      <div class="flex justify-center mb-5">
        <div class="oa-tab-group">
          <button
            class="oa-tab-item"
            data-active={rightTab === 'tasks'}
            onclick={() => handleRightTabChange('tasks')}
          >
            Tasks
            {#if activeTaskCount > 0}
              <span class="oa-count-badge">{activeTaskCount}</span>
            {/if}
          </button>
          <button
            class="oa-tab-item"
            data-active={rightTab === 'agents'}
            onclick={() => handleRightTabChange('agents')}
          >
            Agents
            {#if instanceCount > 0}
              <span class="oa-count-badge">{instanceCount}</span>
            {/if}
          </button>
        </div>
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
