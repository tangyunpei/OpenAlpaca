<script lang="ts">
  import { fade, fly } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import EventLog from "./EventLog.svelte";
  import ConnectorPanel from "./ConnectorPanel.svelte";
  import ConversationsPanel from "./ConversationsPanel.svelte";
  import SettingsPanel from "./SettingsPanel.svelte";
  import AgentConfigPanel from "./AgentConfigPanel.svelte";
  import type { ServerEvent } from "$lib/daemon";

  interface Props {
    open: boolean;
    onClose: () => void;
    eventList: ServerEvent[];
    connectionState: string;
  }

  let { open, onClose, eventList, connectionState }: Props = $props();

  let drawerTab = $state<"settings" | "agents" | "connectors" | "conversations" | "events">("settings");
  let connectorPanel: ConnectorPanel | undefined = $state();
  let settingsPanel: SettingsPanel | undefined = $state();
  let agentConfigPanel: AgentConfigPanel | undefined = $state();

  const drawerTabs = [
    { id: "settings" as const, label: "Configuration" },
    { id: "agents" as const, label: "Agents" },
    { id: "connectors" as const, label: "Connectors" },
    { id: "conversations" as const, label: "Conversations" },
    { id: "events" as const, label: "Event Log" },
  ];

  function handleDrawerTabChange(id: typeof drawerTab) {
    drawerTab = id;
    if (id === "connectors") connectorPanel?.refreshConnectors();
    if (id === "settings") settingsPanel?.refreshSettings();
    if (id === "agents") agentConfigPanel?.refreshConfig();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") onClose();
  }

  $effect(() => {
    if (open) {
      document.body.style.overflow = "hidden";
    } else {
      document.body.style.overflow = "";
    }
    return () => {
      document.body.style.overflow = "";
    };
  });
</script>

{#if open}
  <!-- Backdrop -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    transition:fade={{ duration: 200 }}
    class="fixed inset-0 z-40 bg-[--color-drawer-backdrop] backdrop-blur-sm"
    onclick={onClose}
    onkeydown={handleKeydown}
    role="button"
    tabindex="-1"
    aria-label="Close settings panel"
  ></div>

  <!-- Drawer panel -->
  <div
    transition:fly={{ x: 480, duration: 300, easing: cubicOut }}
    class="fixed top-0 right-0 z-50 h-full w-[480px] max-w-[90vw] flex flex-col bg-card/95 backdrop-blur-2xl border-l border-border"
    style="box-shadow: -8px 0 32px rgba(0, 0, 0, 0.4);"
  >
    <!-- Drawer header -->
    <div class="flex items-center justify-between px-5 py-4 border-b border-border bg-white/2 shrink-0">
      <h2 class="m-0 text-lg font-semibold text-foreground">Settings</h2>
      <button
        class="flex items-center justify-center w-8 h-8 rounded-lg bg-transparent text-muted-foreground hover:bg-white/10 hover:text-foreground transition-colors cursor-pointer border-none"
        onclick={onClose}
        aria-label="Close settings panel"
      >
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M18 6 6 18" />
          <path d="m6 6 12 12" />
        </svg>
      </button>
    </div>

    <!-- Internal tab nav -->
    <div class="flex px-4 pt-1 gap-1 border-b border-border shrink-0 overflow-x-auto flex-nowrap">
      {#each drawerTabs as tab}
        <button
          class="px-3 py-2.5 text-xs font-medium border-b-2 transition-colors whitespace-nowrap cursor-pointer bg-transparent border-x-0 border-t-0
                 {drawerTab === tab.id
                   ? 'border-accent text-accent'
                   : 'border-transparent text-muted-foreground hover:text-foreground'}"
          onclick={() => handleDrawerTabChange(tab.id)}
        >
          {tab.label}
        </button>
      {/each}
    </div>

    <!-- Tab content -->
    <div class="flex-1 overflow-y-auto p-5 min-h-0">
      {#if drawerTab === "events"}
        <EventLog events={eventList} />
      {:else if drawerTab === "agents"}
        <AgentConfigPanel bind:this={agentConfigPanel} />
      {:else if drawerTab === "connectors"}
        <ConnectorPanel bind:this={connectorPanel} connectionState={connectionState} />
      {:else if drawerTab === "conversations"}
        <ConversationsPanel />
      {:else if drawerTab === "settings"}
        <SettingsPanel bind:this={settingsPanel} />
      {/if}
    </div>
  </div>
{/if}
