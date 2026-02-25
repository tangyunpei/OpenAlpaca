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

<div class="mb-5">
  <!-- Template Header -->
  <div class="flex items-center gap-3 px-4 py-3.5 rounded-xl border mb-3 transition-all duration-250
              {isLead ? 'border-accent/15' : 'border-white/[0.05] hover:border-white/8'}"
       style={isLead
         ? 'background: linear-gradient(135deg, hsl(40 85% 58% / 0.06) 0%, hsl(40 85% 58% / 0.02) 100%); border-top: 2px solid hsl(40 85% 58% / 0.3);'
         : 'background: linear-gradient(135deg, rgba(255,255,255,0.025) 0%, rgba(255,255,255,0.01) 100%);'}>
    <div class="w-9 h-9 flex items-center justify-center rounded-xl shrink-0 border
                {isLead ? 'border-accent/15 text-accent' : 'border-white/5 text-muted-foreground'}"
         style={isLead
           ? 'background: linear-gradient(135deg, hsl(40 85% 58% / 0.12) 0%, hsl(40 85% 58% / 0.04) 100%);'
           : 'background: linear-gradient(135deg, rgba(255,255,255,0.05) 0%, rgba(255,255,255,0.02) 100%);'}>
      <span class="text-lg">{resolveIcon(template.icon)}</span>
    </div>
    <div class="flex-1 min-w-0">
      <h3 class="m-0 text-sm font-semibold text-foreground truncate" style="letter-spacing: -0.005em;">{template.name}</h3>
      {#if template.description}
        <p class="m-0 text-[0.7rem] text-muted-foreground/70 truncate mt-0.5">{template.description}</p>
      {/if}
    </div>
    {#if instances.length > 0}
      <span class="text-[0.65rem] px-2.5 py-1 rounded-lg font-medium shrink-0 flex items-center gap-1.5 border border-accent/15"
            style="background: linear-gradient(135deg, hsl(40 85% 58% / 0.1) 0%, hsl(40 85% 58% / 0.04) 100%); color: hsl(40 85% 58%);">
        <span class="w-1.5 h-1.5 rounded-full bg-accent inline-block"></span>
        {instances.length} active
      </span>
    {/if}
    {#if template.model}
      <span class="text-[0.58rem] px-1.5 py-0.5 rounded-md shrink-0 font-mono text-muted-foreground/60 border border-white/[0.04]"
            style="background: rgba(255,255,255,0.025); letter-spacing: 0.01em;">
        {template.model}
      </span>
    {/if}
    {#if template.singleton}
      <span class="text-[0.6rem] px-1.5 py-0.5 rounded-md shrink-0 text-muted-foreground/60 border border-white/[0.04]"
            style="background: rgba(255,255,255,0.025); letter-spacing: 0.03em;">
        singleton
      </span>
    {/if}
  </div>

  <!-- Instance Cards Grid -->
  {#if instances.length > 0}
    <div class="grid grid-cols-[repeat(auto-fill,minmax(130px,1fr))] gap-3 pl-3">
      {#each instances as instance, idx (instance.id)}
        <div style="animation: scaleIn 0.3s cubic-bezier(0.16, 1, 0.3, 1) both; animation-delay: {idx * 40}ms;">
          <InstanceCard {instance} />
        </div>
      {/each}
    </div>
  {:else}
    <div class="text-[0.72rem] text-muted-foreground/40 italic pl-5 py-2.5">
      No active instances
    </div>
  {/if}
</div>
