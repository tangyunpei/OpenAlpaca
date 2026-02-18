<script lang="ts">
  import type { AgentTemplate, AgentInstance } from "$lib/types";
  import { resolveIcon } from "$lib/utils";
  import InstanceCard from "./InstanceCard.svelte";

  interface Props {
    template: AgentTemplate;
    instances: AgentInstance[];
  }

  let { template, instances }: Props = $props();
</script>

<div class="mb-4">
  <!-- Template Header -->
  <div class="flex items-center gap-2.5 px-3 py-2.5 bg-white/2 rounded-lg border border-white/5 mb-3">
    <div class="w-8 h-8 flex items-center justify-center bg-white/5 rounded-lg shrink-0 text-muted-foreground">
      <span class="text-lg">{resolveIcon(template.icon)}</span>
    </div>
    <div class="flex-1 min-w-0">
      <h3 class="m-0 text-sm font-semibold text-foreground truncate">{template.name}</h3>
      {#if template.description}
        <p class="m-0 text-[0.7rem] text-muted-foreground truncate">{template.description}</p>
      {/if}
    </div>
    {#if instances.length > 0}
      <span class="text-[0.7rem] px-2 py-0.5 rounded-full bg-accent/12 text-accent font-medium shrink-0">
        {instances.length} active
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
    <div class="flex flex-wrap gap-3 pl-2">
      {#each instances as instance (instance.id)}
        <InstanceCard {instance} />
      {/each}
    </div>
  {:else}
    <div class="text-[0.75rem] text-muted-foreground/50 italic pl-4 py-2">
      No active instances
    </div>
  {/if}
</div>
