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

  /** Sort templates so the lead agent (with lead_orchestration skill) always appears first */
  let sortedTemplates = $derived(
    [...templates].sort((a, b) => {
      const aLead = a.skills?.includes("lead_orchestration") ? 1 : 0;
      const bLead = b.skills?.includes("lead_orchestration") ? 1 : 0;
      return bLead - aLead;
    }),
  );

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
<div class="flex items-center justify-between mb-5 gap-3">
  <button
    class="px-4 py-2 text-sm font-medium text-foreground/80 border border-white/5 rounded-xl cursor-pointer transition-all duration-200 hover:border-white/10 disabled:opacity-40 disabled:cursor-not-allowed"
    style="background: linear-gradient(180deg, rgba(255,255,255,0.04) 0%, rgba(255,255,255,0.02) 100%);"
    onclick={refresh}
    disabled={loading}
  >
    {#if loading}
      <span class="flex items-center gap-1.5">
        <span class="w-1.5 h-1.5 rounded-full bg-accent animate-pulse"></span>
        Refreshing
      </span>
    {:else}
      Refresh
    {/if}
  </button>

  {#if orchConfig}
    <div class="flex gap-2 text-xs text-muted-foreground">
      <div class="oa-stat">
        <span class="oa-stat-value">{totalInstances}</span>
        <span class="oa-stat-label">Instances</span>
      </div>
      <div class="oa-stat">
        <span class="oa-stat-value">{orchConfig.active_tasks}</span>
        <span class="oa-stat-label">Tasks</span>
      </div>
      <div class="oa-stat">
        <span class="oa-stat-value">${orchConfig.daily_cost_usd.toFixed(4)}</span>
        <span class="oa-stat-label">Cost</span>
      </div>
    </div>
  {/if}
</div>

<!-- Template Sections -->
<div class="pr-1">
  {#if loading && templates.length === 0}
    <!-- Skeleton placeholders while loading -->
    {#each [0, 1, 2] as i}
      <div class="mb-4" style="animation: slideUp 0.35s cubic-bezier(0.16, 1, 0.3, 1) both; animation-delay: {i * 80}ms;">
        <div class="oa-skeleton h-14 mb-3 rounded-xl"></div>
        <div class="flex gap-3 pl-2">
          <div class="oa-skeleton w-[130px] h-[130px] rounded-2xl"></div>
          <div class="oa-skeleton w-[130px] h-[130px] rounded-2xl"></div>
        </div>
      </div>
    {/each}
  {:else}
    {#each sortedTemplates as template, idx (template.id)}
      <div style="animation: slideUp 0.35s cubic-bezier(0.16, 1, 0.3, 1) both; animation-delay: {idx * 60}ms;">
        <TemplateSection
          {template}
          instances={grouped.get(template.id) || []}
          isLead={template.skills?.includes("lead_orchestration") ?? false}
        />
      </div>
    {:else}
      <div class="flex flex-col items-center justify-center py-16 px-10 text-center animate-fadeIn">
        <div class="w-16 h-16 mb-4 rounded-2xl flex items-center justify-center border border-white/5"
             style="background: linear-gradient(135deg, rgba(255,255,255,0.03) 0%, rgba(255,255,255,0.01) 100%);">
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class="text-muted-foreground/40">
            <path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/>
          </svg>
        </div>
        <p class="text-foreground/60 text-sm font-medium mb-1.5">No agent templates registered</p>
        <p class="text-muted-foreground/40 text-xs leading-relaxed">
          Load agent configs from <code class="bg-white/5 px-1.5 py-px rounded text-[0.85rem] border border-white/[0.04]">config/agents/</code> to get started.
        </p>
      </div>
    {/each}
  {/if}
</div>
