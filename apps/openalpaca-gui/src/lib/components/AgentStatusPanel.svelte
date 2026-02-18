<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    templateList,
    templatesLoading,
    loadTemplates,
  } from "$lib/stores/templates";
  import {
    instancesByTemplate,
    instancesLoading,
    loadInstances,
    subscribeToInstanceEvents,
  } from "$lib/stores/instances";
  import {
    orchestratorConfig,
    loadOrchestratorConfig,
    subscribeToOrchestratorEvents,
  } from "$lib/stores/settings";
  import type { AgentTemplate, AgentInstance, OrchestratorConfigResponse } from "$lib/types";
  import TemplateSection from "./TemplateSection.svelte";

  let templates = $state<AgentTemplate[]>([]);
  let grouped = $state<Map<string, AgentInstance[]>>(new Map());
  let tLoading = $state(false);
  let iLoading = $state(false);
  let orchConfig = $state<OrchestratorConfigResponse | null>(null);

  const unsubTemplates = templateList.subscribe((v) => (templates = v));
  const unsubGrouped = instancesByTemplate.subscribe((v) => (grouped = v));
  const unsubTLoading = templatesLoading.subscribe((v) => (tLoading = v));
  const unsubILoading = instancesLoading.subscribe((v) => (iLoading = v));
  const unsubOrch = orchestratorConfig.subscribe((v) => (orchConfig = v));

  let unsubInstanceEvents: (() => void) | null = null;
  let unsubOrchEvents: (() => void) | null = null;

  let loading = $derived(tLoading || iLoading);

  /** Total active instances across all templates */
  let totalInstances = $derived(
    Array.from(grouped.values()).reduce((sum, list) => sum + list.length, 0),
  );

  onMount(() => {
    loadTemplates();
    loadInstances();
    loadOrchestratorConfig();
    unsubInstanceEvents = subscribeToInstanceEvents();
    unsubOrchEvents = subscribeToOrchestratorEvents();
  });

  onDestroy(() => {
    unsubTemplates();
    unsubGrouped();
    unsubTLoading();
    unsubILoading();
    unsubOrch();
    unsubInstanceEvents?.();
    unsubOrchEvents?.();
  });

  async function refresh() {
    await Promise.all([loadTemplates(), loadInstances(), loadOrchestratorConfig()]);
  }
</script>

<!-- Header with stats -->
<div class="flex items-center justify-between mb-5">
  <button
    class="px-4 py-2 text-sm bg-white/5 text-foreground border border-input rounded-lg cursor-pointer hover:bg-white/10 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
    onclick={refresh}
    disabled={loading}
  >
    {loading ? "Refreshing..." : "Refresh"}
  </button>

  {#if orchConfig}
    <div class="flex gap-4 text-xs text-muted-foreground">
      <div class="text-center">
        <span class="block text-sm font-bold text-foreground">{totalInstances}</span>
        <span class="uppercase text-[0.6rem]">Instances</span>
      </div>
      <div class="text-center">
        <span class="block text-sm font-bold text-foreground">{orchConfig.active_tasks}</span>
        <span class="uppercase text-[0.6rem]">Tasks</span>
      </div>
      <div class="text-center">
        <span class="block text-sm font-bold text-foreground">${orchConfig.daily_cost_usd.toFixed(4)}</span>
        <span class="uppercase text-[0.6rem]">Cost</span>
      </div>
    </div>
  {/if}
</div>

<!-- Template Sections -->
<div class="max-h-[calc(100vh-240px)] overflow-y-auto pr-1">
  {#each templates as template (template.id)}
    <TemplateSection
      {template}
      instances={grouped.get(template.id) || []}
    />
  {:else}
    <div class="text-muted-foreground text-center py-15 px-10">
      No agent templates registered. Load agent configs from
      <code class="bg-primary px-1.5 rounded text-[0.85rem]">config/agents/</code>.
    </div>
  {/each}
</div>
