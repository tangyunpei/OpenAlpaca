<script lang="ts">
  import { onDestroy } from "svelte";
  import { agentList, loadAgents, agentsLoading } from "$lib/stores/agents";
  import type { Agent } from "$lib/types";
  import AgentCard from "./AgentCard.svelte";
  import AgentDetail from "./AgentDetail.svelte";
  import AgentCreator from "./AgentCreator.svelte";

  let loading = $state(false);
  let agents = $state<Agent[]>([]);
  let selectedAgentId = $state<string | null>(null);
  let showCreator = $state(false);

  const unsubAgents = agentList.subscribe((v) => (agents = v));
  const unsubLoading = agentsLoading.subscribe((v) => (loading = v));

  onDestroy(() => {
    unsubAgents();
    unsubLoading();
  });

  async function refresh() {
    await loadAgents();
  }
</script>

<div class="flex gap-2.5 mb-5">
  <button
    class="px-4 py-2 text-sm bg-white/5 text-foreground border border-input rounded-lg cursor-pointer hover:bg-white/10 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
    onclick={refresh}
    disabled={loading}
  >
    {loading ? "Refreshing..." : "Refresh"}
  </button>
  <button
    class="px-4 py-2 text-sm bg-accent text-white font-semibold border-none rounded-lg cursor-pointer hover:opacity-90 transition-opacity"
    onclick={() => (showCreator = true)}
  >
    + New Agent
  </button>
</div>

<div class="oa-panel">
  <div class="oa-panel-header">
    <h2>Agents ({agents.length})</h2>
  </div>
  <div class="p-2.5 max-h-[60vh] overflow-y-auto">
    {#each agents as agent (agent.id)}
      <AgentCard {agent} onclick={() => (selectedAgentId = agent.id)} />
    {:else}
      <div class="text-muted-foreground text-center py-15 px-10">
        No agents registered. Load agent configs from
        <code class="bg-primary px-1.5 rounded text-[0.85rem]">config/agents/</code>.
      </div>
    {/each}
  </div>
</div>

{#if selectedAgentId}
  <AgentDetail agentId={selectedAgentId} onClose={() => (selectedAgentId = null)} />
{/if}

{#if showCreator}
  <AgentCreator onClose={() => (showCreator = false)} />
{/if}
