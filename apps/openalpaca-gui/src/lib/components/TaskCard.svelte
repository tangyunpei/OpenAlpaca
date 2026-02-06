<script lang="ts">
  import type { Task } from "$lib/types";
  import { formatTime, getStatusColor } from "$lib/utils";

  interface Props {
    task: Task;
    onclick: () => void;
  }

  let { task, onclick }: Props = $props();

  let hasProgress = $derived(
    task.progress_current != null && task.progress_total != null && task.progress_total > 0,
  );
  let progressPercent = $derived(
    hasProgress ? Math.round((task.progress_current! / task.progress_total!) * 100) : 0,
  );
</script>

<button class="task-card" {onclick}>
  <div class="task-header">
    <h3 class="task-title">{task.title}</h3>
    <span class="status-badge {getStatusColor(task.status)}">{task.status}</span>
  </div>
  {#if hasProgress}
    <div class="progress-bar">
      <div class="progress-fill" style="width: {progressPercent}%"></div>
    </div>
    <span class="progress-text">{task.progress_current}/{task.progress_total}</span>
  {/if}
  {#if task.assigned_agents && task.assigned_agents.length > 0}
    <div class="agents">
      {#each task.assigned_agents as agent}
        <span class="agent-badge">{agent.agent_id}</span>
      {/each}
    </div>
  {/if}
  <div class="task-meta">
    <span class="task-id">{task.id.slice(0, 8)}</span>
    <span class="task-time">{formatTime(task.created_at)}</span>
  </div>
</button>

<style>
  .task-card {
    display: block;
    width: 100%;
    text-align: left;
    padding: 16px 20px;
    background: rgba(255, 255, 255, 0.03);
    border-radius: 10px;
    margin-bottom: 10px;
    border: 1px solid rgba(255, 255, 255, 0.05);
    transition: all 0.2s;
    cursor: pointer;
    color: var(--text);
    font-family: inherit;
    font-size: inherit;
  }

  .task-card:hover {
    background: rgba(255, 255, 255, 0.06);
    transform: translateX(4px);
    border-color: var(--primary);
  }

  .task-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 8px;
  }

  .task-title {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin-right: 12px;
  }

  .status-badge {
    font-size: 0.7rem;
    padding: 2px 8px;
    border-radius: 20px;
    text-transform: uppercase;
    font-weight: 700;
    flex-shrink: 0;
  }

  :global(.status-success) {
    background: rgba(16, 185, 129, 0.2);
    color: var(--success);
  }
  :global(.status-warning) {
    background: rgba(251, 191, 36, 0.2);
    color: #fbbf24;
  }
  :global(.status-paused) {
    background: rgba(96, 165, 250, 0.2);
    color: #60a5fa;
  }
  :global(.status-error) {
    background: rgba(239, 68, 68, 0.2);
    color: var(--error);
  }
  :global(.status-dim) {
    background: rgba(255, 255, 255, 0.05);
    color: var(--text-dim);
  }

  .progress-bar {
    height: 4px;
    background: rgba(255, 255, 255, 0.08);
    border-radius: 2px;
    overflow: hidden;
    margin-bottom: 4px;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 2px;
    transition: width 0.3s ease;
  }

  .progress-text {
    font-size: 0.7rem;
    color: var(--text-dim);
    margin-bottom: 6px;
    display: block;
  }

  .agents {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-bottom: 6px;
  }

  .agent-badge {
    font-size: 0.7rem;
    padding: 1px 8px;
    border-radius: 12px;
    background: rgba(139, 92, 246, 0.2);
    color: #a78bfa;
    font-family: "Fira Code", monospace;
  }

  .task-meta {
    display: flex;
    gap: 12px;
    font-size: 0.75rem;
    color: var(--text-dim);
    font-family: "Fira Code", monospace;
  }

  .task-id {
    background: var(--primary);
    padding: 0 5px;
    border-radius: 3px;
    color: var(--text);
  }
</style>
