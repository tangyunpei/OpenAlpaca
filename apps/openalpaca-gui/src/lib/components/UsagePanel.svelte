<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    usageLogs,
    usageDaily,
    usageLogsLoading,
    usageDailyLoading,
    usageError,
    loadUsageLogs,
    loadUsageDaily,
  } from "$lib/stores/usage";
  import type { LlmCallLog, LlmUsageDaily } from "$lib/types";

  let logs = $state<LlmCallLog[]>([]);
  let daily = $state<LlmUsageDaily[]>([]);
  let logsLoading = $state(false);
  let dailyLoading = $state(false);
  let error = $state<string | null>(null);

  let viewMode = $state<"logs" | "daily">("daily");
  let filterAgent = $state("");

  const unsubLogs = usageLogs.subscribe((v) => (logs = v));
  const unsubDaily = usageDaily.subscribe((v) => (daily = v));
  const unsubLogsLoading = usageLogsLoading.subscribe((v) => (logsLoading = v));
  const unsubDailyLoading = usageDailyLoading.subscribe((v) => (dailyLoading = v));
  const unsubError = usageError.subscribe((v) => (error = v));

  onMount(() => {
    loadUsageDaily();
    loadUsageLogs();
  });

  onDestroy(() => {
    unsubLogs();
    unsubDaily();
    unsubLogsLoading();
    unsubDailyLoading();
    unsubError();
  });

  function handleRefresh() {
    const agent = filterAgent.trim() || undefined;
    loadUsageDaily(agent);
    loadUsageLogs(agent);
  }

  function formatTime(ts: string): string {
    try {
      const d = new Date(ts);
      return d.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" });
    } catch {
      return ts;
    }
  }

  function formatCost(cost: number): string {
    if (cost < 0.001) return `$${cost.toFixed(6)}`;
    return `$${cost.toFixed(4)}`;
  }

  function statusColor(status: string): string {
    switch (status) {
      case "success": return "text-success";
      case "error": return "text-danger";
      default: return "text-muted-foreground";
    }
  }

  let totalCost = $derived(daily.reduce((sum, d) => sum + d.total_cost_usd, 0));
  let totalRequests = $derived(daily.reduce((sum, d) => sum + d.total_requests, 0));
  let totalTokens = $derived(daily.reduce((sum, d) => sum + d.total_input_tokens + d.total_output_tokens, 0));
</script>

<div class="flex flex-col gap-3">
  <div class="flex items-center justify-between">
    <h3 class="m-0 text-base font-semibold text-foreground">LLM Usage</h3>
    <div class="flex items-center gap-2">
      <input
        type="text"
        bind:value={filterAgent}
        placeholder="Filter by agent..."
        class="bg-surface border border-input rounded-md text-foreground px-3 py-1.5 text-[0.8rem] w-40 placeholder:text-muted-foreground focus:outline-none focus:border-accent"
      />
      <button
        class="px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/80 disabled:opacity-50 cursor-pointer"
        onclick={handleRefresh}
        disabled={logsLoading || dailyLoading}
      >
        {logsLoading || dailyLoading ? "Loading..." : "Refresh"}
      </button>
    </div>
  </div>

  {#if error}
    <div class="bg-danger/20 border border-danger text-danger px-3 py-2 rounded-lg text-[0.85rem]">{error}</div>
  {/if}

  <!-- Summary stats -->
  <div class="flex gap-4 bg-card/70 backdrop-blur-xl rounded-[10px] border border-border px-5 py-3.5">
    <div class="text-center flex-1">
      <span class="block text-[1.1rem] font-bold text-foreground">{totalRequests}</span>
      <span class="text-[0.65rem] text-muted-foreground uppercase">Total Requests</span>
    </div>
    <div class="text-center flex-1">
      <span class="block text-[1.1rem] font-bold text-foreground">{(totalTokens / 1000).toFixed(1)}k</span>
      <span class="text-[0.65rem] text-muted-foreground uppercase">Total Tokens</span>
    </div>
    <div class="text-center flex-1">
      <span class="block text-[1.1rem] font-bold text-foreground">{formatCost(totalCost)}</span>
      <span class="text-[0.65rem] text-muted-foreground uppercase">Total Cost</span>
    </div>
  </div>

  <!-- View toggle -->
  <div class="flex gap-1">
    <button
      class="px-4 py-1.5 border border-input rounded-md text-[0.8rem] cursor-pointer transition-all {viewMode === 'daily' ? 'bg-primary text-foreground border-accent' : 'bg-transparent text-muted-foreground hover:bg-white/5'}"
      onclick={() => (viewMode = "daily")}
    >Daily</button>
    <button
      class="px-4 py-1.5 border border-input rounded-md text-[0.8rem] cursor-pointer transition-all {viewMode === 'logs' ? 'bg-primary text-foreground border-accent' : 'bg-transparent text-muted-foreground hover:bg-white/5'}"
      onclick={() => (viewMode = "logs")}
    >Call Log</button>
  </div>

  {#if viewMode === "daily"}
    <!-- Daily aggregate table -->
    <div class="overflow-x-auto rounded-[10px] border border-border bg-card/50">
      {#if dailyLoading && daily.length === 0}
        <div class="text-muted-foreground text-center py-8 text-[0.9rem]">Loading daily usage...</div>
      {:else if daily.length === 0}
        <div class="text-muted-foreground text-center py-8 text-[0.9rem]">No usage data yet.</div>
      {:else}
        <table class="w-full border-collapse text-[0.78rem]">
          <thead>
            <tr>
              <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Date</th>
              <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Agent</th>
              <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Model</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Requests</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">In Tokens</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Out Tokens</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Cost</th>
            </tr>
          </thead>
          <tbody>
            {#each daily as row (row.date + row.agent_id + row.model)}
              <tr class="hover:bg-white/3">
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap font-mono text-xs">{row.date}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap">{row.agent_id}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap font-mono text-xs">{row.model}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono">{row.total_requests}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono">{row.total_input_tokens.toLocaleString()}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono">{row.total_output_tokens.toLocaleString()}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-accent whitespace-nowrap text-right font-mono">{formatCost(row.total_cost_usd)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  {:else}
    <!-- Call log table -->
    <div class="overflow-x-auto rounded-[10px] border border-border bg-card/50">
      {#if logsLoading && logs.length === 0}
        <div class="text-muted-foreground text-center py-8 text-[0.9rem]">Loading call logs...</div>
      {:else if logs.length === 0}
        <div class="text-muted-foreground text-center py-8 text-[0.9rem]">No call logs yet.</div>
      {:else}
        <table class="w-full border-collapse text-[0.78rem]">
          <thead>
            <tr>
              <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Time</th>
              <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Agent</th>
              <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Model</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">In</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Out</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Cost</th>
              <th class="text-right px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Latency</th>
              <th class="text-left px-2.5 py-2 text-[0.7rem] uppercase tracking-wide text-muted-foreground border-b border-border bg-white/2 whitespace-nowrap">Status</th>
            </tr>
          </thead>
          <tbody>
            {#each logs as log (log.id)}
              <tr class="hover:bg-white/3">
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap font-mono text-xs">{formatTime(log.timestamp)}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap">{log.agent_id || "-"}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap font-mono text-xs">{log.model}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono">{log.input_tokens.toLocaleString()}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono">{log.output_tokens.toLocaleString()}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-accent whitespace-nowrap text-right font-mono">{formatCost(log.cost_usd)}</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 text-foreground whitespace-nowrap text-right font-mono">{log.latency_ms ?? "-"}ms</td>
                <td class="px-2.5 py-1.5 border-b border-white/3 whitespace-nowrap">
                  <span class="text-xs font-medium {statusColor(log.status)}">{log.status}</span>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  {/if}
</div>
