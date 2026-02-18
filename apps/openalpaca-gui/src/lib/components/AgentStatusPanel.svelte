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
    <div class="flex gap-2 text-xs text-muted-foreground">
      <div class="bg-white/[0.03] rounded-lg px-3 py-1.5 border border-white/5 text-center min-w-[60px]">
        <span class="block text-sm font-bold text-foreground">{totalInstances}</span>
        <span class="uppercase text-[0.6rem] tracking-wide">Instances</span>
      </div>
      <div class="bg-white/[0.03] rounded-lg px-3 py-1.5 border border-white/5 text-center min-w-[60px]">
        <span class="block text-sm font-bold text-foreground">{orchConfig.active_tasks}</span>
        <span class="uppercase text-[0.6rem] tracking-wide">Tasks</span>
      </div>
      <div class="bg-white/[0.03] rounded-lg px-3 py-1.5 border border-white/5 text-center min-w-[60px]">
        <span class="block text-sm font-bold text-foreground">${orchConfig.daily_cost_usd.toFixed(4)}</span>
        <span class="uppercase text-[0.6rem] tracking-wide">Cost</span>
      </div>
    </div>
  {/if}
</div>

<!-- Template Sections -->
<div class="max-h-[calc(100vh-240px)] overflow-y-auto pr-1">
  {#if loading && templates.length === 0}
    <!-- Skeleton placeholders while loading -->
    {#each [0, 1, 2] as i}
      <div class="mb-4" style="animation: slideUp 0.3s ease-out both; animation-delay: {i * 80}ms;">
        <div class="oa-skeleton h-14 mb-3"></div>
        <div class="flex gap-3 pl-2">
          <div class="oa-skeleton w-[120px] h-[120px]"></div>
          <div class="oa-skeleton w-[120px] h-[120px]"></div>
        </div>
      </div>
    {/each}
  {:else}
    {#each templates as template, idx (template.id)}
      <div style="animation: slideUp 0.3s ease-out both; animation-delay: {idx * 60}ms;">
        <TemplateSection
          {template}
          instances={grouped.get(template.id) || []}
        />
      </div>
    {:else}
      <div class="flex flex-col items-center justify-center py-16 px-10 text-center">
        <div class="w-14 h-14 mb-4 rounded-2xl bg-white/[0.03] border border-white/5 flex items-center justify-center">
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class="text-muted-foreground/50">
            <path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/>
          </svg>
        </div>
        <p class="text-muted-foreground text-sm mb-1">No agent templates registered</p>
        <p class="text-muted-foreground/60 text-xs">
          Load agent configs from <code class="bg-white/5 px-1.5 rounded text-[0.85rem]">config/agents/</code> to get started.
        </p>
      </div>
    {/each}
  {/if}
</div>
