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

  const drawerTabs: { id: typeof drawerTab; label: string; icon: string }[] = [
    { id: "settings", label: "Configuration", icon: "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" },
    { id: "agents", label: "Agents", icon: "M12 8V4H8 M4 8h16v12H4z M2 14h2 M20 14h2 M15 13v2 M9 13v2" },
    { id: "connectors", label: "Connectors", icon: "M12 2v6m0 8v6M6 12H2m20 0h-4M7.8 7.8 4.6 4.6m14.8 14.8-3.2-3.2M7.8 16.2l-3.2 3.2M19.4 4.6l-3.2 3.2" },
    { id: "conversations", label: "Conversations", icon: "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" },
    { id: "events", label: "Event Log", icon: "M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z M13 2v7h7" },
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
          class="px-3 py-2.5 text-xs font-medium border-b-2 transition-colors whitespace-nowrap cursor-pointer bg-transparent border-x-0 border-t-0 flex items-center gap-1.5
                 {drawerTab === tab.id
                   ? 'border-accent text-accent'
                   : 'border-transparent text-muted-foreground hover:text-foreground'}"
          onclick={() => handleDrawerTabChange(tab.id)}
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="shrink-0">
            <path d={tab.icon} />
          </svg>
          {tab.label}
        </button>
      {/each}
    </div>

    <!-- Tab content -->
    <div class="flex-1 overflow-y-auto p-5 min-h-0">
      {#key drawerTab}
        <div class="animate-fadeIn">
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
      {/key}
    </div>
  </div>
{/if}
