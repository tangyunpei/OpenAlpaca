<script lang="ts">
  import type { AgentTemplate, AgentInstance } from "$lib/types";
  import { resolveIcon } from "$lib/utils";
  import InstanceCard from "./InstanceCard.svelte";

  interface Props {
    template: AgentTemplate;
    instances: AgentInstance[];
    isLead?: boolean;
  }

  let { template, instances, isLead = false }: Props = $props();
</script>

<div class="mb-4">
  <!-- Template Header -->
  <div class="flex items-center gap-2.5 px-3.5 py-3 rounded-xl border border-white/5 mb-3 transition-all duration-200 hover:shadow-sm
              {isLead ? 'bg-white/[0.04] border-t-2 border-t-accent' : 'bg-white/2 hover:bg-white/[0.04]'}">
    <div class="w-8 h-8 flex items-center justify-center rounded-lg shrink-0
                {isLead ? 'bg-accent/10 text-accent' : 'bg-white/5 text-muted-foreground'}">
      <span class="text-lg">{resolveIcon(template.icon)}</span>
    </div>
    <div class="flex-1 min-w-0">
      <h3 class="m-0 text-sm font-semibold text-foreground truncate">{template.name}</h3>
      {#if template.description}
        <p class="m-0 text-[0.7rem] text-muted-foreground truncate">{template.description}</p>
      {/if}
    </div>
    {#if instances.length > 0}
      <span class="text-[0.7rem] px-2 py-0.5 rounded-full bg-accent/15 text-accent font-medium shrink-0 flex items-center gap-1">
        <span class="w-1.5 h-1.5 rounded-full bg-accent inline-block"></span>
        {instances.length} active
      </span>
    {/if}
    {#if template.model}
      <span class="text-[0.6rem] px-1.5 py-0.5 rounded bg-white/5 text-muted-foreground/70 shrink-0 font-mono">
        {template.model}
      </span>
    {/if}
    {#if template.singleton}
      <span class="text-[0.65rem] px-1.5 py-0.5 rounded bg-white/5 text-muted-foreground shrink-0">
        singleton
      </span>
    {/if}
  </div>

  <!-- Instance Cards Grid -->
  {#if instances.length > 0}
    <div class="grid grid-cols-[repeat(auto-fill,minmax(120px,1fr))] gap-3 pl-2">
      {#each instances as instance (instance.id)}
        <InstanceCard {instance} />
      {/each}
    </div>
  {:else}
    <div class="text-[0.75rem] text-muted-foreground/60 italic pl-4 py-2">
      No active instances
    </div>
  {/if}
</div>
