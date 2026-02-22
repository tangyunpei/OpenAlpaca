<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    decisionRecords,
    decisionsLoading,
    decisionsError,
    loadDecisions,
  } from "$lib/stores/decisions";
  import type { DispatchDecisionRecord } from "$lib/types";

  let records = $state<DispatchDecisionRecord[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  let filterMode = $state("");

  const unsubRecords = decisionRecords.subscribe((v) => (records = v));
  const unsubLoading = decisionsLoading.subscribe((v) => (loading = v));
  const unsubError = decisionsError.subscribe((v) => (error = v));

  onMount(() => {
    loadDecisions();
  });

  onDestroy(() => {
    unsubRecords();
    unsubLoading();
    unsubError();
  });

  function handleRefresh() {
    const mode = filterMode.trim() || undefined;
    loadDecisions(mode);
  }

  function formatTime(ts: string): string {
    try {
      const d = new Date(ts);
      return d.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" });
    } catch {
      return ts;
    }
  }

  function modeColor(mode: string): string {
    switch (mode) {
      case "lead_agent": return "text-accent";
      case "dag_parallel": return "text-success";
      case "sequential_pipeline": return "text-muted-foreground";
      default: return "text-foreground";
    }
  }

  function modeLabel(mode: string): string {
    switch (mode) {
      case "lead_agent": return "Lead Agent";
      case "dag_parallel": return "DAG Parallel";
      case "sequential_pipeline": return "Pipeline";
      default: return mode;
    }
  }

  function reasonLabel(reason: string): string {
    switch (reason) {
      case "planner_explicit": return "Planner";
      case "execution_mode_field": return "V2 Mode";
      case "empty_assignments_fallback": return "Fallback";
      case "heuristic_fallback": return "Heuristic";
      default: return reason;
    }
  }

  let totalDecisions = $derived(records.length);
  let leadAgentCount = $derived(records.filter((r) => r.mode === "lead_agent").length);
  let dagCount = $derived(records.filter((r) => r.mode === "dag_parallel").length);
  let pipelineCount = $derived(records.filter((r) => r.mode === "sequential_pipeline").length);
  let avgPredictability = $derived(() => {
    const scored = records.filter((r) => r.predictability_score !== null);
    if (scored.length === 0) return null;
    return scored.reduce((sum, r) => sum + (r.predictability_score ?? 0), 0) / scored.length;
  });
</script>

<div class="flex flex-col gap-3">
  <div class="flex items-center justify-between">
    <h3 class="m-0 text-base font-semibold text-foreground">Dispatch Decisions</h3>
    <div class="flex items-center gap-2">
      <input
        type="text"
        bind:value={filterMode}
        placeholder="Filter by mode..."
        class="bg-surface border border-input rounded-md text-foreground px-3 py-1.5 text-[0.8rem] w-40 placeholder:text-muted-foreground focus:outline-none focus:border-accent"
      />
      <button
        class="px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/80 disabled:opacity-50 cursor-pointer"
        onclick={handleRefresh}
        disabled={loading}
      >
        {loading ? "Loading..." : "Refresh"}
      </button>
    </div>
  </div>

  {#if error}
    <div class="bg-danger/20 border border-danger text-danger px-3 py-2 rounded-lg text-[0.85rem]">{error}</div>
  {/if}

  <!-- Summary stats -->
  <div class="flex gap-4 bg-card/70 backdrop-blur-xl rounded-[10px] border border-border px-5 py-3.5">
    <div class="text-center flex-1">
      <span class="block text-[1.1rem] font-bold text-foreground">{totalDecisions}</span>
      <span class="text-[0.65rem] text-muted-foreground uppercase">Total</span>
    </div>
    <div class="text-center flex-1">
      <span class="block text-[1.1rem] font-bold text-accent">{leadAgentCount}</span>
      <span class="text-[0.65rem] text-muted-foreground uppercase">Lead Agent</span>
    </div>
    <div class="text-center flex-1">
      <span class="block text-[1.1rem] font-bold text-success">{dagCount}</span>
      <span class="text-[0.65rem] text-muted-foreground uppercase">DAG</span>
    </div>
    <div class="text-center flex-1">
      <span class="block text-[1.1rem] font-bold text-foreground">{pipelineCount}</span>
      <span class="text-[0.65rem] text-muted-foreground uppercase">Pipeline</span>
    </div>
    <div class="text-center flex-1">
      <span class="block text-[1.1rem] font-bold text-foreground">{avgPredictability() !== null ? avgPredictability()!.toFixed(2) : "-"}</span>
      <span class="text-[0.65rem] text-muted-foreground uppercase">Avg Score</span>
    </div>
  </div>

  <!-- Records table -->
  <div class="overflow-x-auto rounded-[10px] border border-border bg-card/50">
    {#if loading && records.length === 0}
      <div class="text-muted-foreground text-center py-8 text-[0.9rem]">Loading decisions...</div>
    {:else if records.length === 0}
      <div class="text-muted-foreground text-center py-8 text-[0.9rem]">No dispatch decisions yet.</div>
    {:else}
      <table class="w-full border-collapse text-[0.78rem]">
        <thead>
          <tr>
            <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Time</th>
            <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Request</th>
            <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Task</th>
            <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Mode</th>
            <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Reason</th>
            <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Agents</th>
            <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">DAG Nodes</th>
            <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Score</th>
            <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Planner Mode</th>
          </tr>
        </thead>
        <tbody>
          {#each records as rec (rec.id)}
            <tr class="hover:bg-white/3">
              <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap font-mono text-xs">{formatTime(rec.timestamp)}</td>
              <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap font-mono text-xs">{rec.request_id.slice(0, 8)}...</td>
              <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap font-mono text-xs">{rec.task_id ? rec.task_id.slice(0, 8) + "..." : "\u2014"}</td>
              <td class="px-2.5 py-1.5 border-b border-white/3 whitespace-nowrap">
                <span class="text-xs font-medium {modeColor(rec.mode)}">{modeLabel(rec.mode)}</span>
              </td>
              <td class="px-2.5 py-1.5 border-b border-white/3 text-muted-foreground whitespace-nowrap text-xs">{reasonLabel(rec.reason)}</td>
              <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono">{rec.agent_count}</td>
              <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono">{rec.dag_node_count ?? "-"}</td>
              <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono">{rec.predictability_score !== null ? rec.predictability_score.toFixed(2) : "-"}</td>
              <td class="px-2.5 py-1.5 border-b border-white/3 text-muted-foreground whitespace-nowrap text-xs">{rec.planner_requested_mode ?? "-"}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
</div>
