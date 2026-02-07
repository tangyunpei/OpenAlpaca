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
      case "success": return "var(--success)";
      case "error": return "var(--error)";
      default: return "var(--text-dim)";
    }
  }

  /** Compute summary stats from daily data */
  let totalCost = $derived(daily.reduce((sum, d) => sum + d.total_cost_usd, 0));
  let totalRequests = $derived(daily.reduce((sum, d) => sum + d.total_requests, 0));
  let totalTokens = $derived(daily.reduce((sum, d) => sum + d.total_input_tokens + d.total_output_tokens, 0));
</script>

<div class="usage-panel">
  <div class="usage-header">
    <h3>LLM Usage</h3>
    <div class="header-actions">
      <input
        type="text"
        class="filter-input"
        bind:value={filterAgent}
        placeholder="Filter by agent..."
      />
      <button onclick={handleRefresh} disabled={logsLoading || dailyLoading}>
        {logsLoading || dailyLoading ? "Loading..." : "Refresh"}
      </button>
    </div>
  </div>

  {#if error}
    <div class="usage-error">{error}</div>
  {/if}

  <!-- Summary stats -->
  <div class="stats-row">
    <div class="stat">
      <span class="stat-value">{totalRequests}</span>
      <span class="stat-label">Total Requests</span>
    </div>
    <div class="stat">
      <span class="stat-value">{(totalTokens / 1000).toFixed(1)}k</span>
      <span class="stat-label">Total Tokens</span>
    </div>
    <div class="stat">
      <span class="stat-value">{formatCost(totalCost)}</span>
      <span class="stat-label">Total Cost</span>
    </div>
  </div>

  <!-- View toggle -->
  <div class="view-toggle">
    <button
      class="filter-btn"
      class:active={viewMode === "daily"}
      onclick={() => (viewMode = "daily")}
    >Daily</button>
    <button
      class="filter-btn"
      class:active={viewMode === "logs"}
      onclick={() => (viewMode = "logs")}
    >Call Log</button>
  </div>

  {#if viewMode === "daily"}
    <!-- Daily aggregate table -->
    <div class="table-wrapper">
      {#if dailyLoading && daily.length === 0}
        <div class="loading">Loading daily usage...</div>
      {:else if daily.length === 0}
        <div class="empty">No usage data yet.</div>
      {:else}
        <table class="usage-table">
          <thead>
            <tr>
              <th>Date</th>
              <th>Agent</th>
              <th>Model</th>
              <th class="num">Requests</th>
              <th class="num">In Tokens</th>
              <th class="num">Out Tokens</th>
              <th class="num">Cost</th>
            </tr>
          </thead>
          <tbody>
            {#each daily as row (row.date + row.agent_id + row.model)}
              <tr>
                <td class="mono">{row.date}</td>
                <td>{row.agent_id}</td>
                <td class="mono">{row.model}</td>
                <td class="num">{row.total_requests}</td>
                <td class="num">{row.total_input_tokens.toLocaleString()}</td>
                <td class="num">{row.total_output_tokens.toLocaleString()}</td>
                <td class="num cost">{formatCost(row.total_cost_usd)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  {:else}
    <!-- Call log table -->
    <div class="table-wrapper">
      {#if logsLoading && logs.length === 0}
        <div class="loading">Loading call logs...</div>
      {:else if logs.length === 0}
        <div class="empty">No call logs yet.</div>
      {:else}
        <table class="usage-table">
          <thead>
            <tr>
              <th>Time</th>
              <th>Agent</th>
              <th>Model</th>
              <th class="num">In</th>
              <th class="num">Out</th>
              <th class="num">Cost</th>
              <th class="num">Latency</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody>
            {#each logs as log (log.id)}
              <tr>
                <td class="mono">{formatTime(log.timestamp)}</td>
                <td>{log.agent_id || "-"}</td>
                <td class="mono">{log.model}</td>
                <td class="num">{log.input_tokens.toLocaleString()}</td>
                <td class="num">{log.output_tokens.toLocaleString()}</td>
                <td class="num cost">{formatCost(log.cost_usd)}</td>
                <td class="num">{log.latency_ms ?? "-"}ms</td>
                <td>
                  <span class="status-dot" style="color: {statusColor(log.status)}">{log.status}</span>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  {/if}
</div>

<style>
  .usage-panel {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .usage-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .usage-header h3 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
  }

  .header-actions {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .filter-input {
    background: var(--surface);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 6px;
    color: var(--text);
    padding: 6px 12px;
    font-size: 0.8rem;
    width: 160px;
  }
  .filter-input:focus { outline: none; border-color: var(--accent); }
  .filter-input::placeholder { color: var(--text-dim); }

  .usage-error {
    background: rgba(239, 68, 68, 0.2);
    border: 1px solid var(--error);
    color: var(--error);
    padding: 8px 12px;
    border-radius: 8px;
    font-size: 0.85rem;
  }

  .stats-row {
    display: flex;
    gap: 16px;
    background: rgba(30, 30, 50, 0.7);
    backdrop-filter: blur(20px);
    -webkit-backdrop-filter: blur(20px);
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.08);
    padding: 14px 20px;
  }

  .stat {
    text-align: center;
    flex: 1;
  }

  .stat-value {
    display: block;
    font-size: 1.1rem;
    font-weight: 700;
    color: var(--text);
  }

  .stat-label {
    font-size: 0.65rem;
    color: var(--text-dim);
    text-transform: uppercase;
  }

  .view-toggle {
    display: flex;
    gap: 4px;
  }

  .view-toggle .filter-btn {
    padding: 6px 16px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 6px;
    background: transparent;
    color: var(--text-dim);
    font-size: 0.8rem;
    cursor: pointer;
    transition: all 0.15s;
  }

  .view-toggle .filter-btn.active {
    background: var(--primary);
    color: var(--text);
    border-color: var(--accent);
  }

  .view-toggle .filter-btn:hover:not(.active) {
    background: rgba(255, 255, 255, 0.05);
  }

  .table-wrapper {
    overflow-x: auto;
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.08);
    background: rgba(30, 30, 50, 0.5);
  }

  .usage-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.78rem;
  }

  .usage-table th {
    text-align: left;
    padding: 8px 10px;
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.3px;
    color: var(--text-dim);
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
    background: rgba(255, 255, 255, 0.02);
    white-space: nowrap;
  }

  .usage-table td {
    padding: 6px 10px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.03);
    color: var(--text);
    white-space: nowrap;
  }

  .usage-table tbody tr:hover {
    background: rgba(255, 255, 255, 0.03);
  }

  .usage-table .num {
    text-align: right;
    font-family: "Fira Code", monospace;
  }

  .usage-table .mono {
    font-family: "Fira Code", monospace;
    font-size: 0.75rem;
  }

  .cost {
    color: var(--accent);
  }

  .status-dot {
    font-size: 0.75rem;
    font-weight: 500;
  }

  .loading, .empty {
    color: var(--text-dim);
    text-align: center;
    padding: 30px;
    font-size: 0.9rem;
  }
</style>
