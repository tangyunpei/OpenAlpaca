<script lang="ts">
  import ApiKeyManager from "./ApiKeyManager.svelte";
  import AddKeyDialog from "./AddKeyDialog.svelte";
  import { cliBackends } from "$lib/stores/settings";
  import type { ProviderInfo, CliBackendStatus } from "$lib/types";

  interface Props {
    providerName: string;
    provider: ProviderInfo;
    onRefresh: () => void;
  }

  let { providerName, provider, onRefresh }: Props = $props();

  let showAddDialog = $state(false);
  let backends = $state<CliBackendStatus[]>([]);

  const unsubBackends = cliBackends.subscribe((v) => (backends = v));

  function providerDisplayName(name: string): string {
    switch (name) {
      case "anthropic": return "Anthropic (Claude)";
      case "openai": return "OpenAI (GPT)";
      case "ollama": return "Ollama (Local)";
      default: return name;
    }
  }

  function handleKeyAdded() {
    showAddDialog = false;
    onRefresh();
  }

  /** Match provider name to relevant CLI backend */
  function relevantBackend(providerName: string): CliBackendStatus | null {
    if (providerName === "anthropic") return backends.find(b => b.name === "claude_code") ?? null;
    if (providerName === "openai") return backends.find(b => b.name === "codex") ?? null;
    return null;
  }

  $effect(() => {
    return () => unsubBackends();
  });
</script>

<div class="provider-section">
  <div class="provider-header">
    <div class="provider-title">
      <h3>{providerDisplayName(providerName)}</h3>
      <span class="status-badge" class:active={provider.enabled} class:disabled={!provider.enabled}>
        {provider.enabled ? "Enabled" : "Disabled"}
      </span>
      <span class="strategy-badge">{provider.key_selection_strategy.replace(/_/g, ' ')}</span>
    </div>
    <button class="add-key-btn" onclick={() => (showAddDialog = true)}>
      + Add Key
    </button>
  </div>

  <ApiKeyManager
    provider={providerName}
    keys={provider.keys}
    {onRefresh}
  />

  {#if relevantBackend(providerName)}
    {@const backend = relevantBackend(providerName)}
    {#if backend}
      <div class="cli-fallback-row">
        <span class="cli-dot" class:cli-available={backend.available} class:cli-unavailable={!backend.available}></span>
        <span class="cli-label">CLI Fallback:</span>
        {#if backend.available}
          <span class="cli-path">{backend.name} detected{backend.path ? ` at ${backend.path}` : ""}</span>
          {#if !backend.enabled}
            <span class="cli-disabled-badge">disabled</span>
          {/if}
        {:else}
          <span class="cli-not-found">{backend.name} not found</span>
        {/if}
      </div>
    {/if}
  {/if}
</div>

{#if showAddDialog}
  <AddKeyDialog
    provider={providerName}
    onclose={() => (showAddDialog = false)}
    onAdded={handleKeyAdded}
  />
{/if}

<style>
  .provider-section {
    background: rgba(30, 30, 50, 0.7);
    backdrop-filter: blur(20px);
    -webkit-backdrop-filter: blur(20px);
    border-radius: 12px;
    padding: 16px;
    border: 1px solid rgba(255, 255, 255, 0.08);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.2);
  }

  .provider-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 12px;
    flex-wrap: wrap;
    gap: 8px;
  }

  .provider-title {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .provider-title h3 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
    color: var(--text);
  }

  .status-badge {
    font-size: 0.65rem;
    padding: 2px 8px;
    border-radius: 20px;
    text-transform: uppercase;
    font-weight: 700;
  }

  .status-badge.active {
    background: rgba(16, 185, 129, 0.2);
    color: var(--success);
  }

  .status-badge.disabled {
    background: rgba(107, 114, 128, 0.2);
    color: #9ca3af;
  }

  .strategy-badge {
    font-size: 0.65rem;
    padding: 2px 8px;
    border-radius: 20px;
    background: rgba(15, 52, 96, 0.4);
    color: var(--text-dim);
    text-transform: capitalize;
  }

  .add-key-btn {
    padding: 6px 14px !important;
    font-size: 0.8rem !important;
    border-radius: 6px !important;
    white-space: nowrap;
  }

  .cli-fallback-row {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid rgba(255, 255, 255, 0.05);
    font-size: 0.75rem;
  }

  .cli-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .cli-available { background: var(--success); }
  .cli-unavailable { background: var(--text-dim); }

  .cli-label {
    color: var(--text-dim);
  }

  .cli-path {
    color: var(--text);
    font-family: monospace;
    font-size: 0.7rem;
  }

  .cli-not-found {
    color: var(--text-dim);
    opacity: 0.6;
  }

  .cli-disabled-badge {
    font-size: 0.6rem;
    padding: 0px 4px;
    border-radius: 2px;
    background: rgba(245, 158, 11, 0.15);
    color: #f59e0b;
    text-transform: uppercase;
  }
</style>
