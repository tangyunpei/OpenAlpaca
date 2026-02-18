<script lang="ts">
  import { onMount } from "svelte";
  import ProviderConfig from "./ProviderConfig.svelte";
  import WebSearchConfig from "./WebSearchConfig.svelte";
  import UsagePanel from "./UsagePanel.svelte";
  import {
    llmSettings,
    settingsLoading,
    settingsError,
    providerList,
    availableModels,
    modelsRefreshing,
    loadSettings,
    loadAvailableModels,
    loadDiscoveredCredentials,
    loadCliBackends,
    refreshModels,
    subscribeToKeyEvents,
    loadDaemonProviders,
  } from "$lib/stores/settings";
  import type { LlmSettingsResponse, ProviderInfo, ModelEntry } from "$lib/types";

  let settings = $state<LlmSettingsResponse | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let providers = $state<[string, ProviderInfo][]>([]);
  let models = $state<ModelEntry[]>([]);
  let refreshing = $state(false);

  let settingsTab = $state<"config" | "usage">("config");

  const unsubSettings = llmSettings.subscribe((v) => (settings = v));
  const unsubLoading = settingsLoading.subscribe((v) => (loading = v));
  const unsubError = settingsError.subscribe((v) => (error = v));
  const unsubProviders = providerList.subscribe((v) => (providers = v));
  const unsubModels = availableModels.subscribe((v) => (models = v));
  const unsubRefreshing = modelsRefreshing.subscribe((v) => (refreshing = v));

  let unsubKeyEvents: (() => void) | null = null;

  onMount(() => {
    loadSettings();
    loadAvailableModels();
    loadDiscoveredCredentials();
    loadCliBackends();
    loadDaemonProviders();
    unsubKeyEvents = subscribeToKeyEvents();

    return () => {
      unsubSettings();
      unsubLoading();
      unsubError();
      unsubProviders();
      unsubModels();
      unsubRefreshing();
      unsubKeyEvents?.();
    };
  });

  function handleModelsRefresh() {
    loadAvailableModels();
    refreshModels();
  }

  export function refreshSettings() {
    loadSettings();
    loadAvailableModels();
    loadDiscoveredCredentials();
    loadCliBackends();
    loadDaemonProviders();
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
      onclick={() => { loadSettings(); loadAvailableModels(); }}
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
  <div class="flex flex-col gap-4">
    {#each providers as [name, prov] (name)}
      <ProviderConfig
        providerName={name}
        provider={prov}
        onRefresh={() => loadSettings()}
        onModelsRefresh={handleModelsRefresh}
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

  <!-- Service Providers -->
  <h3 class="text-sm font-semibold text-muted-foreground mt-6 mb-3">Service Providers</h3>
  <WebSearchConfig />
{/if}
