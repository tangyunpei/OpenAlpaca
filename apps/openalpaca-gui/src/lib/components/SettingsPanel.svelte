<script lang="ts">
  import { onMount } from "svelte";
  import ProviderConfig from "./ProviderConfig.svelte";
  import {
    llmSettings,
    settingsLoading,
    settingsError,
    providerList,
    loadSettings,
    subscribeToKeyEvents,
  } from "$lib/stores/settings";
  import type { LlmSettingsResponse, ProviderInfo } from "$lib/types";

  let settings = $state<LlmSettingsResponse | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let providers = $state<[string, ProviderInfo][]>([]);

  const unsubSettings = llmSettings.subscribe((v) => (settings = v));
  const unsubLoading = settingsLoading.subscribe((v) => (loading = v));
  const unsubError = settingsError.subscribe((v) => (error = v));
  const unsubProviders = providerList.subscribe((v) => (providers = v));

  let unsubKeyEvents: (() => void) | null = null;

  onMount(() => {
    loadSettings();
    unsubKeyEvents = subscribeToKeyEvents();

    return () => {
      unsubSettings();
      unsubLoading();
      unsubError();
      unsubProviders();
      unsubKeyEvents?.();
    };
  });

  export function refreshSettings() {
    loadSettings();
  }
</script>

<div class="controls">
  <button onclick={() => loadSettings()} disabled={loading}>
    {loading ? "Loading..." : "Refresh"}
  </button>
</div>

{#if error}
  <div class="settings-error">{error}</div>
{/if}

{#if settings}
  <div class="orchestrator-info">
    <div class="panel-header">
      <h2>Orchestrator</h2>
    </div>
    <div class="orchestrator-body">
      <div class="info-row">
        <span class="label">Default Model</span>
        <span class="value">{settings.orchestrator.model}</span>
      </div>
      {#if settings.orchestrator.fallback_models.length > 0}
        <div class="info-row">
          <span class="label">Fallback Models</span>
          <span class="value">{settings.orchestrator.fallback_models.join(', ')}</span>
        </div>
      {/if}
    </div>
  </div>

  <div class="providers-grid">
    {#each providers as [name, prov] (name)}
      <ProviderConfig
        providerName={name}
        provider={prov}
        onRefresh={() => loadSettings()}
      />
    {:else}
      <div class="empty">No LLM providers configured.</div>
    {/each}
  </div>
{:else if !loading}
  <div class="empty">
    LLM not configured. Add a <code>config/llm.toml</code> file to get started.
  </div>
{/if}

<style>
  .controls {
    display: flex;
    gap: 10px;
    margin-bottom: 20px;
  }

  .settings-error {
    background: rgba(239, 68, 68, 0.2);
    border: 1px solid var(--error);
    color: var(--error);
    padding: 10px 14px;
    border-radius: 8px;
    margin-bottom: 16px;
    font-size: 0.9rem;
  }

  .orchestrator-info {
    background: rgba(30, 30, 50, 0.7);
    backdrop-filter: blur(20px);
    -webkit-backdrop-filter: blur(20px);
    border-radius: 12px;
    overflow: hidden;
    border: 1px solid rgba(255, 255, 255, 0.08);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.2);
    margin-bottom: 20px;
  }

  .panel-header {
    padding: 12px 16px;
    background: rgba(255, 255, 255, 0.02);
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
  }

  .panel-header h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
  }

  .orchestrator-body {
    padding: 12px 16px;
  }

  .info-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 6px 0;
  }

  .info-row .label {
    font-size: 0.85rem;
    color: var(--text-dim);
  }

  .info-row .value {
    font-size: 0.85rem;
    color: var(--text);
    font-weight: 500;
    font-family: monospace;
  }

  .providers-grid {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .empty {
    color: var(--text-dim);
    text-align: center;
    padding: 40px;
    font-size: 0.95rem;
  }

  .empty code {
    background: rgba(255, 255, 255, 0.1);
    padding: 2px 6px;
    border-radius: 3px;
  }
</style>
