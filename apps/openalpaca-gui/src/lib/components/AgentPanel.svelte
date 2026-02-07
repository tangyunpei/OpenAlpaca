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

<div class="controls">
  <button onclick={refresh} disabled={loading}>
    {loading ? "Refreshing..." : "Refresh"}
  </button>
  <button class="new-agent-btn" onclick={() => (showCreator = true)}>
    + New Agent
  </button>
</div>

<div class="view-panel">
  <div class="panel-header">
    <h2>Agents ({agents.length})</h2>
  </div>
  <div class="agent-list">
    {#each agents as agent (agent.id)}
      <AgentCard {agent} onclick={() => (selectedAgentId = agent.id)} />
    {:else}
      <div class="empty">
        No agents registered. Load agent configs from <code>config/agents/</code>.
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

<style>
  .controls {
    display: flex;
    gap: 10px;
    margin-bottom: 20px;
  }

  .new-agent-btn {
    background: var(--accent);
    color: white;
    font-weight: 600;
  }
  .new-agent-btn:hover {
    opacity: 0.9;
  }

  .view-panel {
    background: rgba(30, 30, 50, 0.7);
    backdrop-filter: blur(20px);
    -webkit-backdrop-filter: blur(20px);
    border-radius: 16px;
    padding: 0;
    overflow: hidden;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }

  .panel-header {
    padding: 15px 20px;
    background: rgba(255, 255, 255, 0.02);
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
  }

  .view-panel h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
    color: var(--text);
  }

  .agent-list {
    padding: 10px;
    max-height: 60vh;
    overflow-y: auto;
  }

  .agent-list::-webkit-scrollbar {
    width: 6px;
  }
  .agent-list::-webkit-scrollbar-track {
    background: transparent;
  }
  .agent-list::-webkit-scrollbar-thumb {
    background: var(--primary);
    border-radius: 3px;
  }

  .empty {
    color: var(--text-dim);
    text-align: center;
    padding: 60px 40px;
  }

  .empty code {
    background: var(--primary);
    padding: 1px 6px;
    border-radius: 4px;
    font-size: 0.85rem;
  }
</style>
