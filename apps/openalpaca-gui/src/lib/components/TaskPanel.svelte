<script lang="ts">
  import { activeTasks, completedTasks, loadTasks, tasksLoading } from "$lib/stores/tasks";
  import type { Task } from "$lib/types";
  import TaskCard from "./TaskCard.svelte";
  import TaskDetail from "./TaskDetail.svelte";

  let filter = $state<"active" | "completed">("active");
  let loading = $state(false);
  let displayedTasks = $state<Task[]>([]);
  let selectedTaskId = $state<string | null>(null);

  // Single $effect manages subscription lifecycle — cleans up on filter change or destroy
  $effect(() => {
    const store = filter === "active" ? activeTasks : completedTasks;
    const unsub = store.subscribe((v) => (displayedTasks = v));
    return unsub;
  });

  $effect(() => {
    const unsub = tasksLoading.subscribe((v) => (loading = v));
    return unsub;
  });

  function setFilter(f: "active" | "completed") {
    filter = f;
  }

  async function refresh() {
    await loadTasks();
  }
</script>

<div class="controls">
  <button onclick={refresh} disabled={loading}>
    {loading ? "Refreshing..." : "Refresh"}
  </button>
  <div class="filter-toggle">
    <button
      class="filter-btn"
      class:active={filter === "active"}
      onclick={() => setFilter("active")}
    >Active</button>
    <button
      class="filter-btn"
      class:active={filter === "completed"}
      onclick={() => setFilter("completed")}
    >Completed</button>
  </div>
</div>

<div class="view-panel">
  <div class="panel-header">
    <h2>Tasks ({displayedTasks.length})</h2>
  </div>
  <div class="task-list">
    {#each displayedTasks as task (task.id)}
      <TaskCard {task} onclick={() => (selectedTaskId = task.id)} />
    {:else}
      <div class="empty">
        {filter === "active" ? "No active tasks." : "No completed tasks."}
      </div>
    {/each}
  </div>
</div>

{#if selectedTaskId}
  <TaskDetail taskId={selectedTaskId} onClose={() => (selectedTaskId = null)} />
{/if}

<style>
  .controls {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-bottom: 20px;
    align-items: center;
  }

  .filter-toggle {
    display: flex;
    background: rgba(0, 0, 0, 0.25);
    padding: 3px;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.05);
  }

  .filter-btn {
    padding: 6px 16px;
    border: none;
    background: transparent;
    color: var(--text-dim);
    cursor: pointer;
    font-size: 0.8rem;
    font-weight: 500;
    border-radius: 5px;
    transition: all 0.2s;
  }

  .filter-btn:hover {
    color: var(--text);
  }

  .filter-btn.active {
    background: rgba(255, 255, 255, 0.1);
    color: #fff;
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

  .task-list {
    padding: 10px;
    max-height: 60vh;
    overflow-y: auto;
  }

  .task-list::-webkit-scrollbar {
    width: 6px;
  }
  .task-list::-webkit-scrollbar-track {
    background: transparent;
  }
  .task-list::-webkit-scrollbar-thumb {
    background: var(--primary);
    border-radius: 3px;
  }

  .empty {
    color: var(--text-dim);
    text-align: center;
    padding: 60px 40px;
  }
</style>
