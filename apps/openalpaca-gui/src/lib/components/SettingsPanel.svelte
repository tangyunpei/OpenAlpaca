<script lang="ts">
  import { onMount } from "svelte";
  import ProviderConfig from "./ProviderConfig.svelte";
  import UsagePanel from "./UsagePanel.svelte";
  import {
    llmSettings,
    settingsLoading,
    settingsError,
    providerList,
    orchestratorConfig,
    availableModels,
    modelsRefreshing,
    loadSettings,
    loadOrchestratorConfig,
    loadAvailableModels,
    refreshModels,
    saveOrchestratorConfig,
    subscribeToKeyEvents,
    subscribeToOrchestratorEvents,
  } from "$lib/stores/settings";
  import type { LlmSettingsResponse, ProviderInfo, OrchestratorConfigResponse, ModelEntry } from "$lib/types";

  let settings = $state<LlmSettingsResponse | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let providers = $state<[string, ProviderInfo][]>([]);
  let orchConfig = $state<OrchestratorConfigResponse | null>(null);
  let models = $state<ModelEntry[]>([]);
  let refreshing = $state(false);

  // Sub-tab: Config vs Usage
  let settingsTab = $state<"config" | "usage">("config");

  // Edit mode state
  let editingOrchestrator = $state(false);
  let editModel = $state("");
  let editFallbackSet = $state<Set<string>>(new Set());
  let orchSaving = $state(false);
  let orchError = $state<string | null>(null);

  const unsubSettings = llmSettings.subscribe((v) => (settings = v));
  const unsubLoading = settingsLoading.subscribe((v) => (loading = v));
  const unsubError = settingsError.subscribe((v) => (error = v));
  const unsubProviders = providerList.subscribe((v) => (providers = v));
  const unsubOrch = orchestratorConfig.subscribe((v) => {
    orchConfig = v;
    if (v && !editingOrchestrator) {
      editModel = v.model;
      editFallbackSet = new Set(v.fallback_models);
    }
  });
  const unsubModels = availableModels.subscribe((v) => (models = v));
  const unsubRefreshing = modelsRefreshing.subscribe((v) => (refreshing = v));

  let unsubKeyEvents: (() => void) | null = null;
  let unsubOrchEvents: (() => void) | null = null;

  onMount(() => {
    loadSettings();
    loadOrchestratorConfig();
    loadAvailableModels();
    unsubKeyEvents = subscribeToKeyEvents();
    unsubOrchEvents = subscribeToOrchestratorEvents();

    return () => {
      unsubSettings();
      unsubLoading();
      unsubError();
      unsubProviders();
      unsubOrch();
      unsubModels();
      unsubRefreshing();
      unsubKeyEvents?.();
      unsubOrchEvents?.();
    };
  });

  function startEditOrchestrator() {
    if (orchConfig) {
      editModel = orchConfig.model;
      editFallbackSet = new Set(orchConfig.fallback_models);
    }
    editingOrchestrator = true;
    orchError = null;
  }

  function toggleFallback(modelId: string) {
    const next = new Set(editFallbackSet);
    if (next.has(modelId)) {
      next.delete(modelId);
    } else {
      next.add(modelId);
    }
    editFallbackSet = next;
  }

  function cancelEditOrchestrator() {
    editingOrchestrator = false;
    orchError = null;
  }

  async function handleSaveOrchestrator() {
    orchSaving = true;
    orchError = null;
    try {
      await saveOrchestratorConfig(editModel, [...editFallbackSet]);
      editingOrchestrator = false;
    } catch (e) {
      orchError = e instanceof Error ? e.message : String(e);
    } finally {
      orchSaving = false;
    }
  }

  export function refreshSettings() {
    loadSettings();
    loadOrchestratorConfig();
    loadAvailableModels();
  }
</script>

<div class="settings-tabs">
  <button
    class="filter-btn"
    class:active={settingsTab === "config"}
    onclick={() => (settingsTab = "config")}
  >Configuration</button>
  <button
    class="filter-btn"
    class:active={settingsTab === "usage"}
    onclick={() => (settingsTab = "usage")}
  >Usage</button>
</div>

{#if settingsTab === "usage"}
  <UsagePanel />
{:else}
  <div class="controls">
    <button onclick={() => { loadSettings(); loadOrchestratorConfig(); loadAvailableModels(); }} disabled={loading}>
      {loading ? "Loading..." : "Refresh"}
    </button>
    <button onclick={refreshModels} disabled={refreshing} class="secondary">
      {refreshing ? "Refreshing..." : "Refresh Models"}
    </button>
  </div>

  {#if error}
    <div class="settings-error">{error}</div>
  {/if}

  {#if settings}
  <div class="orchestrator-info">
    <div class="panel-header">
      <h2>Orchestrator</h2>
      {#if !editingOrchestrator}
        <button class="edit-btn" onclick={startEditOrchestrator}>Edit</button>
      {/if}
    </div>
    <div class="orchestrator-body">
      {#if editingOrchestrator}
        {#if orchError}
          <div class="orch-error">{orchError}</div>
        {/if}
        <div class="edit-row">
          <label>
            <span class="label">Model</span>
            {#if models.length > 0}
              <select bind:value={editModel}>
                {#each models as m (m.id)}
                  <option value={m.id}>{m.id} ({m.provider})</option>
                {/each}
                {#if editModel && !models.find((m) => m.id === editModel)}
                  <option value={editModel}>{editModel} (current)</option>
                {/if}
              </select>
            {:else}
              <input type="text" bind:value={editModel} />
            {/if}
          </label>
        </div>
        <div class="edit-row">
          <span class="label">Fallback Models</span>
          {#if models.length > 0}
            <div class="checkbox-list">
              {#each models.filter((m) => m.id !== editModel) as m (m.id)}
                <label class="checkbox-item">
                  <input
                    type="checkbox"
                    checked={editFallbackSet.has(m.id)}
                    onchange={() => toggleFallback(m.id)}
                  />
                  <span>{m.id} <span class="provider-tag">{m.provider}</span></span>
                </label>
              {/each}
            </div>
          {:else}
            <input type="text" value={[...editFallbackSet].join(", ")} onchange={(e) => {
              editFallbackSet = new Set(e.currentTarget.value.split(",").map((s) => s.trim()).filter(Boolean));
            }} placeholder="gpt-4o, llama3" />
          {/if}
        </div>
        <div class="edit-actions">
          <button onclick={handleSaveOrchestrator} disabled={orchSaving}>
            {orchSaving ? "Saving..." : "Save"}
          </button>
          <button class="secondary" onclick={cancelEditOrchestrator}>Cancel</button>
        </div>
      {:else}
        <div class="info-row">
          <span class="label">Default Model</span>
          <span class="value">{orchConfig?.model || settings.orchestrator.model}</span>
        </div>
        {#if (orchConfig?.fallback_models || settings.orchestrator.fallback_models).length > 0}
          <div class="info-row">
            <span class="label">Fallback Models</span>
            <span class="value">{(orchConfig?.fallback_models || settings.orchestrator.fallback_models).join(', ')}</span>
          </div>
        {/if}
        {#if orchConfig}
          <div class="stats-row">
            <div class="stat">
              <span class="stat-value">{orchConfig.active_agents}</span>
              <span class="stat-label">Agents</span>
            </div>
            <div class="stat">
              <span class="stat-value">{orchConfig.active_tasks}</span>
              <span class="stat-label">Active Tasks</span>
            </div>
            <div class="stat">
              <span class="stat-value">${orchConfig.daily_cost_usd.toFixed(4)}</span>
              <span class="stat-label">Cost</span>
            </div>
          </div>
        {/if}
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
{/if}

<style>
  .settings-tabs {
    display: flex;
    gap: 4px;
    margin-bottom: 16px;
  }

  .settings-tabs .filter-btn {
    padding: 6px 20px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 6px;
    background: transparent;
    color: var(--text-dim);
    font-size: 0.85rem;
    cursor: pointer;
    transition: all 0.15s;
  }

  .settings-tabs .filter-btn.active {
    background: var(--primary);
    color: var(--text);
    border-color: var(--accent);
  }

  .settings-tabs .filter-btn:hover:not(.active) {
    background: rgba(255, 255, 255, 0.05);
  }

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
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .panel-header h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
  }

  .edit-btn {
    font-size: 0.75rem;
    padding: 4px 12px;
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

  .stats-row {
    display: flex;
    gap: 16px;
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid rgba(255, 255, 255, 0.05);
  }
  .stat {
    text-align: center;
    flex: 1;
  }
  .stat-value {
    display: block;
    font-size: 1rem;
    font-weight: 700;
    color: var(--text);
  }
  .stat-label {
    font-size: 0.65rem;
    color: var(--text-dim);
    text-transform: uppercase;
  }

  .edit-row {
    margin-bottom: 10px;
  }
  .edit-row > label {
    display: flex; flex-direction: column; gap: 4px;
  }
  .edit-row .label {
    font-size: 0.75rem; color: var(--text-dim);
    text-transform: uppercase; letter-spacing: 0.3px;
  }
  .edit-row input, .edit-row select {
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 6px; padding: 6px 10px;
    color: var(--text); font-size: 0.85rem;
  }
  .edit-row input:focus, .edit-row select:focus { outline: none; border-color: var(--accent); }
  .edit-row select { cursor: pointer; }
  .edit-row select option { background: var(--surface); color: var(--text); }

  .checkbox-list {
    display: flex; flex-direction: column; gap: 4px;
    max-height: 180px; overflow-y: auto;
    padding: 6px 0;
  }
  .checkbox-item {
    display: flex; align-items: center; gap: 8px;
    font-size: 0.82rem; color: var(--text);
    cursor: pointer; padding: 2px 0;
  }
  .checkbox-item input[type="checkbox"] {
    width: 14px; height: 14px; cursor: pointer;
    accent-color: var(--accent);
  }
  .provider-tag {
    font-size: 0.7rem; color: var(--text-dim);
    background: rgba(255, 255, 255, 0.06);
    padding: 1px 6px; border-radius: 3px;
    margin-left: 4px;
  }

  .edit-actions {
    display: flex; gap: 8px; justify-content: flex-end; margin-top: 10px;
  }

  .orch-error {
    background: rgba(239, 68, 68, 0.2);
    border: 1px solid var(--error);
    color: var(--error);
    padding: 8px 12px; border-radius: 6px; margin-bottom: 10px;
    font-size: 0.8rem;
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
