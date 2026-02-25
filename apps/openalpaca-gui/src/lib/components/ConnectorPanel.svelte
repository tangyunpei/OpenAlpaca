<script lang="ts">
  import {
    getConnectors,
    performConnectorAction,
    generateLinkToken,
    configureConnector,
    type Connector,
  } from "$lib/connectors";

  interface Props {
    connectionState: string;
  }

  let { connectionState }: Props = $props();

  let connectorsList = $state<Connector[]>([]);
  let linkToken = $state<string | null>(null);
  let isLoadingConnectors = $state(false);

  let showConfigModal = $state(false);
  let configTargetId = $state<string | null>(null);
  let configToken = $state("");

  // Auto-load connectors when the component mounts and connection is active
  $effect(() => {
    if (connectionState === "connected" && connectorsList.length === 0 && !isLoadingConnectors) {
      refreshConnectors();
    }
  });

  export async function refreshConnectors() {
    if (connectionState !== "connected") return;
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

  async function handleAction(id: string, action: "enable" | "disable" | "delete") {
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

  function getStatusLabel(status: string): string {
    switch (status) {
      case "unconfigured": return "Not Configured";
      case "active": return "Active";
      case "disabled": return "Disabled";
      case "error": return "Error";
      default: return status;
    }
  }

  const statusBadgeClass: Record<string, string> = {
    active: "bg-success/20 text-success",
    error: "bg-danger/20 text-danger",
    disabled: "bg-gray-500/20 text-gray-400",
    unconfigured: "bg-white/5 text-muted-foreground",
  };
</script>

<div class="flex flex-wrap gap-2.5 mb-5 max-[480px]:flex-wrap">
  <button
    class="inline-flex items-center justify-center px-5 py-2 text-sm font-medium rounded-lg bg-primary text-foreground hover:bg-accent hover:text-accent-foreground transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
    onclick={refreshConnectors}
    disabled={isLoadingConnectors}
  >
    {isLoadingConnectors ? "Refreshing..." : "Refresh"}
  </button>
  <button
    class="inline-flex items-center justify-center px-5 py-2 text-sm font-medium rounded-lg bg-primary text-foreground hover:bg-accent hover:text-accent-foreground transition-all cursor-pointer"
    onclick={handleGenerateToken}
  >Generate Bind Token</button>
</div>

{#if linkToken}
  <div class="bg-gradient-to-br from-primary to-[#1a3a5f] p-5 rounded-xl mb-5 text-center border border-accent">
    <p class="text-foreground">Binding Code (expires in 5m):</p>
    <div class="bg-black/30 p-4 rounded-lg text-3xl tracking-widest my-2.5 text-accent font-extrabold">
      <code>{linkToken}</code>
    </div>
    <p class="text-sm text-muted-foreground">
      Send <code class="bg-white/10 px-1.5 rounded">/link {linkToken}</code> to your Telegram bot or via iMessage.
    </p>
  </div>
{/if}

<div class="oa-panel">
  <div class="oa-panel-header">
    <h2>Platform Connectors</h2>
  </div>
  <div class="p-2.5">
    {#each connectorsList as connector}
      <div class="flex justify-between items-center px-5 py-4 bg-white/3 rounded-[10px] mb-2.5 border border-white/5 transition-all hover:bg-white/6 hover:translate-x-1 hover:border-primary flex-wrap gap-2.5">
        <div>
          <h3 class="m-0 mb-1 text-lg text-foreground">{connector.name}</h3>
          <span class="text-xs px-2 py-0.5 rounded-full uppercase font-bold {statusBadgeClass[connector.status] || 'bg-white/5 text-muted-foreground'}">
            {getStatusLabel(connector.status)}
          </span>
        </div>
        <div class="flex items-center gap-4">
          <button
            class="action-btn w-8 h-8 flex items-center justify-center bg-white/3 border border-white/8 rounded-lg text-muted-foreground cursor-pointer transition-all hover:bg-card hover:border-primary hover:text-primary hover:-translate-y-px"
            title="Configure"
            onclick={() => openConfigModal(connector.id)}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
              <circle cx="12" cy="12" r="3" />
            </svg>
          </button>

          <label class="relative inline-block w-11 h-6 shrink-0">
            <input
              type="checkbox"
              checked={connector.status === "active"}
              onchange={() => handleToggle(connector.id, connector.status === "active")}
              class="peer sr-only"
            />
            <span class="block h-6 w-11 rounded-full bg-primary cursor-pointer transition-colors duration-300 peer-checked:bg-success"></span>
            <span class="absolute left-0.5 top-0.5 h-5 w-5 rounded-full bg-white transition-transform duration-300 peer-checked:translate-x-5"></span>
          </label>

          <button
            class="action-btn w-8 h-8 flex items-center justify-center bg-danger/10 border border-white/8 rounded-lg text-danger cursor-pointer transition-all hover:bg-danger hover:text-white hover:-translate-y-px"
            title="Clear Config"
            onclick={() => handleAction(connector.id, "delete")}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="M3 6h18m-2 0v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2m-6 5v6m4-6v6" />
            </svg>
          </button>
        </div>
      </div>
    {:else}
      <div class="text-muted-foreground text-center py-15 px-10">
        No connectors found. Check your configuration.
      </div>
    {/each}
  </div>
</div>

{#if showConfigModal}
  <div class="fixed inset-0 bg-black/60 flex justify-center items-center z-50">
    <div class="bg-card p-6 rounded-xl w-[90%] max-w-[400px] shadow-2xl border border-border">
      <h3 class="mt-0 text-primary text-lg">Configure {configTargetId}</h3>
      <p class="text-sm text-muted-foreground">Enter the authentication token for this connector.</p>
      <input
        type="password"
        bind:value={configToken}
        placeholder="Token (e.g. 12345:ABC...)"
        class="w-full p-2.5 my-4 bg-background border border-border text-foreground rounded-md"
      />
      <div class="flex justify-end gap-2.5">
        <button
          class="px-5 py-2 text-sm rounded-lg bg-white/5 text-muted-foreground cursor-pointer hover:bg-white/10 transition-colors"
          onclick={() => (showConfigModal = false)}
        >Cancel</button>
        <button
          class="px-5 py-2 text-sm rounded-lg bg-primary text-foreground cursor-pointer hover:bg-accent hover:text-accent-foreground transition-colors"
          onclick={handleConfigSubmit}
        >Save & Enable</button>
      </div>
    </div>
  </div>
{/if}
