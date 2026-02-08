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
    loadDiscoveredCredentials,
    loadCliBackends,
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

  let settingsTab = $state<"config" | "usage">("config");

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
    loadDiscoveredCredentials();
    loadCliBackends();
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
    loadDiscoveredCredentials();
    loadCliBackends();
  }
</script>

<div class="flex gap-1 mb-4">
  <button
    class="px-5 py-1.5 border border-input rounded-md text-[0.85rem] cursor-pointer transition-all {settingsTab === 'config' ? 'bg-primary text-foreground border-accent' : 'bg-transparent text-muted-foreground hover:bg-white/5'}"
    onclick={() => (settingsTab = "config")}
  >Configuration</button>
  <button
    class="px-5 py-1.5 border border-input rounded-md text-[0.85rem] cursor-pointer transition-all {settingsTab === 'usage' ? 'bg-primary text-foreground border-accent' : 'bg-transparent text-muted-foreground hover:bg-white/5'}"
    onclick={() => (settingsTab = "usage")}
  >Usage</button>
</div>

{#if settingsTab === "usage"}
  <UsagePanel />
{:else}
  <div class="flex gap-2.5 mb-5">
    <button
      class="px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/80 disabled:opacity-50 cursor-pointer"
      onclick={() => { loadSettings(); loadOrchestratorConfig(); loadAvailableModels(); }}
      disabled={loading}
    >
      {loading ? "Loading..." : "Refresh"}
    </button>
    <button
      class="px-4 py-2 rounded-lg bg-white/5 border border-border text-muted-foreground text-sm hover:bg-white/10 cursor-pointer disabled:opacity-50"
      onclick={refreshModels}
      disabled={refreshing}
    >
      {refreshing ? "Refreshing..." : "Refresh Models"}
    </button>
  </div>

  {#if error}
    <div class="bg-danger/20 border border-danger text-danger px-3.5 py-2.5 rounded-lg mb-4 text-[0.9rem]">{error}</div>
  {/if}

  {#if settings}
  <div class="bg-card/70 backdrop-blur-xl rounded-xl overflow-hidden border border-border shadow-[0_4px_16px_rgba(0,0,0,0.2)] mb-5">
    <div class="flex items-center justify-between px-4 py-3 bg-white/2 border-b border-border">
      <h2 class="m-0 text-base font-semibold text-foreground">Orchestrator</h2>
      {#if !editingOrchestrator}
        <button
          class="text-xs px-3 py-1 rounded-md bg-white/5 border border-border text-muted-foreground hover:bg-white/10 cursor-pointer"
          onclick={startEditOrchestrator}
        >Edit</button>
      {/if}
    </div>
    <div class="px-4 py-3">
      {#if editingOrchestrator}
        {#if orchError}
          <div class="bg-danger/20 border border-danger text-danger px-3 py-2 rounded-md mb-2.5 text-[0.8rem]">{orchError}</div>
        {/if}
        <div class="mb-2.5">
          <label class="flex flex-col gap-1">
            <span class="text-xs text-muted-foreground uppercase tracking-wide">Model</span>
            {#if models.length > 0}
              <select bind:value={editModel} class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] cursor-pointer focus:outline-none focus:border-accent">
                {#each models as m (m.id)}
                  <option value={m.id}>{m.id} ({m.provider})</option>
                {/each}
                {#if editModel && !models.find((m) => m.id === editModel)}
                  <option value={editModel}>{editModel} (current)</option>
                {/if}
              </select>
            {:else}
              <input type="text" bind:value={editModel} class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] focus:outline-none focus:border-accent" />
            {/if}
          </label>
        </div>
        <div class="mb-2.5">
          <span class="text-xs text-muted-foreground uppercase tracking-wide">Fallback Models</span>
          {#if models.length > 0}
            <div class="flex flex-col gap-1 max-h-[180px] overflow-y-auto py-1.5">
              {#each models.filter((m) => m.id !== editModel) as m (m.id)}
                <label class="flex items-center gap-2 text-[0.82rem] text-foreground cursor-pointer py-0.5">
                  <input
                    type="checkbox"
                    checked={editFallbackSet.has(m.id)}
                    onchange={() => toggleFallback(m.id)}
                    class="w-3.5 h-3.5 cursor-pointer accent-accent"
                  />
                  <span>{m.id} <span class="text-[0.7rem] text-muted-foreground bg-white/5 px-1.5 rounded ml-1">{m.provider}</span></span>
                </label>
              {/each}
            </div>
          {:else}
            <input type="text" value={[...editFallbackSet].join(", ")} onchange={(e) => {
              editFallbackSet = new Set(e.currentTarget.value.split(",").map((s) => s.trim()).filter(Boolean));
            }} placeholder="gpt-4o, llama3" class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] placeholder:text-muted-foreground focus:outline-none focus:border-accent w-full" />
          {/if}
        </div>
        <div class="flex gap-2 justify-end mt-2.5">
          <button
            class="px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/80 disabled:opacity-50 cursor-pointer"
            onclick={handleSaveOrchestrator}
            disabled={orchSaving}
          >
            {orchSaving ? "Saving..." : "Save"}
          </button>
          <button
            class="px-4 py-2 rounded-lg bg-white/5 border border-border text-muted-foreground text-sm hover:bg-white/10 cursor-pointer"
            onclick={cancelEditOrchestrator}
          >Cancel</button>
        </div>
      {:else}
        <div class="flex items-center justify-between py-1.5">
          <span class="text-[0.85rem] text-muted-foreground">Default Model</span>
          <span class="text-[0.85rem] text-foreground font-medium font-mono">{orchConfig?.model || settings.orchestrator.model}</span>
        </div>
        {#if (orchConfig?.fallback_models || settings.orchestrator.fallback_models).length > 0}
          <div class="flex items-center justify-between py-1.5">
            <span class="text-[0.85rem] text-muted-foreground">Fallback Models</span>
            <span class="text-[0.85rem] text-foreground font-medium font-mono">{(orchConfig?.fallback_models || settings.orchestrator.fallback_models).join(', ')}</span>
          </div>
        {/if}
        {#if orchConfig}
          <div class="flex gap-4 mt-2.5 pt-2.5 border-t border-border">
            <div class="text-center flex-1">
              <span class="block text-base font-bold text-foreground">{orchConfig.active_agents}</span>
              <span class="text-[0.65rem] text-muted-foreground uppercase">Agents</span>
            </div>
            <div class="text-center flex-1">
              <span class="block text-base font-bold text-foreground">{orchConfig.active_tasks}</span>
              <span class="text-[0.65rem] text-muted-foreground uppercase">Active Tasks</span>
            </div>
            <div class="text-center flex-1">
              <span class="block text-base font-bold text-foreground">${orchConfig.daily_cost_usd.toFixed(4)}</span>
              <span class="text-[0.65rem] text-muted-foreground uppercase">Cost</span>
            </div>
          </div>
        {/if}
      {/if}
    </div>
  </div>

  <div class="flex flex-col gap-4">
    {#each providers as [name, prov] (name)}
      <ProviderConfig
        providerName={name}
        provider={prov}
        onRefresh={() => loadSettings()}
      />
    {:else}
      <div class="text-muted-foreground text-center py-10 text-[0.95rem]">No LLM providers configured.</div>
    {/each}
  </div>
{:else if !loading}
  <div class="text-muted-foreground text-center py-10 text-[0.95rem]">
    LLM not configured. Add a <code class="bg-white/10 px-1.5 rounded">config/llm.toml</code> file to get started.
  </div>
{/if}
{/if}
