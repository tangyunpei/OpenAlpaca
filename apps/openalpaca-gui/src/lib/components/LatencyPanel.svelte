<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    latencyRecords,
    latencyAggregates,
    latencyRecordsLoading,
    latencyAggregatesLoading,
    latencyError,
    loadLatencyRecords,
    loadLatencyAggregates,
  } from "$lib/stores/latency";
  import type { OrchestratorLatencyRecord, LatencyAggregate } from "$lib/types";

  let records = $state<OrchestratorLatencyRecord[]>([]);
  let aggregates = $state<LatencyAggregate[]>([]);
  let recordsLoading = $state(false);
  let aggregatesLoading = $state(false);
  let error = $state<string | null>(null);

  let viewMode = $state<"aggregates" | "records">("aggregates");
  let filterMode = $state("");

  const unsubRecords = latencyRecords.subscribe((v) => (records = v));
  const unsubAggregates = latencyAggregates.subscribe((v) => (aggregates = v));
  const unsubRecordsLoading = latencyRecordsLoading.subscribe((v) => (recordsLoading = v));
  const unsubAggregatesLoading = latencyAggregatesLoading.subscribe((v) => (aggregatesLoading = v));
  const unsubError = latencyError.subscribe((v) => (error = v));

  onMount(() => {
    loadLatencyAggregates();
    loadLatencyRecords();
  });

  onDestroy(() => {
    unsubRecords();
    unsubAggregates();
    unsubRecordsLoading();
    unsubAggregatesLoading();
    unsubError();
  });

  function handleRefresh() {
    const mode = filterMode.trim() || undefined;
    loadLatencyAggregates();
    loadLatencyRecords(mode);
  }

  function formatTime(ts: string): string {
    try {
      const d = new Date(ts);
      return d.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" });
    } catch {
      return ts;
    }
  }

  function formatMs(ms: number): string {
    if (ms < 1) return "<1ms";
    return `${ms.toLocaleString()}ms`;
  }

  let totalRecordCount = $derived(aggregates.reduce((sum, a) => sum + a.count, 0));
  let totalAutoPromotions = $derived(aggregates.reduce((sum, a) => sum + a.auto_promotion_count, 0));
  let totalFallbacks = $derived(aggregates.reduce((sum, a) => sum + a.fallback_count, 0));
</script>

<div class="flex flex-col gap-3">
  <div class="flex items-center justify-between">
    <h3 class="m-0 text-base font-semibold text-foreground">Orchestrator Latency</h3>
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
        disabled={recordsLoading || aggregatesLoading}
      >
        {recordsLoading || aggregatesLoading ? "Loading..." : "Refresh"}
      </button>
    </div>
  </div>

  {#if error}
    <div class="bg-danger/20 border border-danger text-danger px-3 py-2 rounded-lg text-[0.85rem]">{error}</div>
  {/if}

  <!-- Summary stats -->
  <div class="flex gap-4 bg-card/70 backdrop-blur-xl rounded-[10px] border border-border px-5 py-3.5">
    <div class="text-center flex-1">
      <span class="block text-[1.1rem] font-bold text-foreground">{totalRecordCount}</span>
      <span class="text-[0.65rem] text-muted-foreground uppercase">Total Records</span>
    </div>
    <div class="text-center flex-1">
      <span class="block text-[1.1rem] font-bold text-foreground">{aggregates.length}</span>
      <span class="text-[0.65rem] text-muted-foreground uppercase">Modes</span>
    </div>
    <div class="text-center flex-1">
      <span class="block text-[1.1rem] font-bold text-accent">{totalAutoPromotions}</span>
      <span class="text-[0.65rem] text-muted-foreground uppercase">Auto-Promotions</span>
    </div>
    <div class="text-center flex-1">
      <span class="block text-[1.1rem] font-bold text-foreground">{totalFallbacks}</span>
      <span class="text-[0.65rem] text-muted-foreground uppercase">Fallbacks</span>
    </div>
  </div>

  <!-- View toggle -->
  <div class="flex gap-1">
    <button
      class="px-4 py-1.5 border border-input rounded-md text-[0.8rem] cursor-pointer transition-all {viewMode === 'aggregates' ? 'bg-primary text-foreground border-accent' : 'bg-transparent text-muted-foreground hover:bg-white/5'}"
      onclick={() => (viewMode = "aggregates")}
    >Aggregates</button>
    <button
      class="px-4 py-1.5 border border-input rounded-md text-[0.8rem] cursor-pointer transition-all {viewMode === 'records' ? 'bg-primary text-foreground border-accent' : 'bg-transparent text-muted-foreground hover:bg-white/5'}"
      onclick={() => (viewMode = "records")}
    >Records</button>
  </div>

  {#if viewMode === "aggregates"}
    <!-- Aggregates table -->
    <div class="overflow-x-auto rounded-[10px] border border-border bg-card/50">
      {#if aggregatesLoading && aggregates.length === 0}
        <div class="text-muted-foreground text-center py-8 text-[0.9rem]">Loading aggregates...</div>
      {:else if aggregates.length === 0}
        <div class="text-muted-foreground text-center py-8 text-[0.9rem]">No latency data yet.</div>
      {:else}
        <table class="w-full border-collapse text-[0.78rem]">
          <thead>
            <tr>
              <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Mode</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Count</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">P50</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">P95</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">P99</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Planner</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Dispatch</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Ack</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Promos</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Fallbacks</th>
            </tr>
          </thead>
          <tbody>
            {#each aggregates as agg (agg.mode)}
              <tr class="hover:bg-white/3">
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap font-medium">{agg.mode}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono">{agg.count}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono text-xs">{formatMs(agg.p50_total_ms)}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono text-xs">{formatMs(agg.p95_total_ms)}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-accent whitespace-nowrap text-right font-mono text-xs">{formatMs(agg.p99_total_ms)}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono text-xs">{agg.mean_planner_ms.toFixed(1)}ms</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono text-xs">{agg.mean_dispatch_ms.toFixed(1)}ms</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono text-xs">{agg.mean_ack_ms.toFixed(1)}ms</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono">{agg.auto_promotion_count}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono">{agg.fallback_count}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  {:else}
    <!-- Records table -->
    <div class="overflow-x-auto rounded-[10px] border border-border bg-card/50">
      {#if recordsLoading && records.length === 0}
        <div class="text-muted-foreground text-center py-8 text-[0.9rem]">Loading records...</div>
      {:else if records.length === 0}
        <div class="text-muted-foreground text-center py-8 text-[0.9rem]">No latency records yet.</div>
      {:else}
        <table class="w-full border-collapse text-[0.78rem]">
          <thead>
            <tr>
              <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Time</th>
              <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Request</th>
              <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Mode</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Planner</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Dispatch</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Ack</th>
              <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Fallback</th>
              <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Auto-promo</th>
            </tr>
          </thead>
          <tbody>
            {#each records as rec (rec.id)}
              <tr class="hover:bg-white/3">
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap font-mono text-xs">{formatTime(rec.timestamp)}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap font-mono text-xs">{rec.request_id.slice(0, 8)}...</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap">{rec.mode}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono text-xs">{formatMs(rec.planner_ms)}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono text-xs">{formatMs(rec.dispatch_ms)}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono text-xs">{formatMs(rec.ack_ms)}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-muted-foreground whitespace-nowrap text-xs">{rec.fallback_reason ?? "-"}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-muted-foreground whitespace-nowrap text-xs">{rec.auto_promotion_reason ?? "-"}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  {/if}
</div>
