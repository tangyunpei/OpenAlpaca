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

  // Configuration modal state
  let showConfigModal = $state(false);
  let configTargetId = $state<string | null>(null);
  let configToken = $state("");

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
</script>

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
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
              <circle cx="12" cy="12" r="3" />
            </svg>
          </button>

          <label class="switch">
            <input
              type="checkbox"
              checked={connector.status === "active"}
              onchange={() => handleToggle(connector.id, connector.status === "active")}
            />
            <span class="slider"></span>
          </label>
          <button
            class="action-btn danger icon"
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
      <div class="empty">
        No connectors found. Check your configuration.
      </div>
    {/each}
  </div>
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
        <button class="secondary" onclick={() => (showConfigModal = false)}>Cancel</button>
        <button onclick={handleConfigSubmit}>Save & Enable</button>
      </div>
    </div>
  </div>
{/if}

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
    flex-wrap: wrap;
    gap: 10px;
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
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.08);
    padding: 6px;
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    border-radius: 8px;
    color: var(--text-dim);
    transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.1);
  }

  .action-btn.icon:hover {
    background: var(--surface);
    border-color: var(--primary);
    color: var(--primary);
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
  }

  .action-btn.danger {
    background: rgba(239, 68, 68, 0.1);
    color: var(--error);
  }
  .action-btn.danger:hover {
    background: var(--error);
    color: white;
  }
  .action-btn.danger.icon:hover {
    border-color: var(--error);
    color: var(--error);
    background: rgba(239, 68, 68, 0.1);
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

  .empty {
    color: var(--text-dim);
    text-align: center;
    padding: 60px 40px;
  }

  /* Modal */
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
    border: 1px solid rgba(255, 255, 255, 0.08);
  }

  .modal h3 {
    margin-top: 0;
    color: var(--primary);
  }

  .token-input {
    width: 100%;
    padding: 10px;
    margin: 15px 0;
    background: var(--bg);
    border: 1px solid rgba(255, 255, 255, 0.08);
    color: var(--text);
    border-radius: 6px;
    box-sizing: border-box;
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }

  @media (max-width: 480px) {
    .controls {
      flex-wrap: wrap;
    }

    .token-box {
      font-size: 1.3rem;
      letter-spacing: 2px;
    }
  }
</style>
