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

<div class="flex flex-wrap gap-2.5 mb-5 items-center">
  <button
    class="px-4 py-2 rounded-lg border border-border bg-card text-sm font-medium text-foreground cursor-pointer transition-all duration-200 hover:bg-white/5 disabled:opacity-50 disabled:cursor-not-allowed"
    onclick={refresh}
    disabled={loading}
  >
    {loading ? "Refreshing..." : "Refresh"}
  </button>
  <div class="flex bg-black/25 p-[3px] rounded-lg border border-white/5">
    <button
      class="px-4 py-1.5 border-none bg-transparent text-muted-foreground cursor-pointer text-[0.8rem] font-medium rounded-[5px] transition-all duration-200 hover:text-foreground {filter === 'active' ? 'bg-white/10 text-white' : ''}"
      onclick={() => setFilter("active")}
    >Active</button>
    <button
      class="px-4 py-1.5 border-none bg-transparent text-muted-foreground cursor-pointer text-[0.8rem] font-medium rounded-[5px] transition-all duration-200 hover:text-foreground {filter === 'completed' ? 'bg-white/10 text-white' : ''}"
      onclick={() => setFilter("completed")}
    >Completed</button>
  </div>
</div>

<div class="oa-panel">
  <div class="oa-panel-header">
    <h2>Tasks ({displayedTasks.length})</h2>
  </div>
  <div class="p-2.5 max-h-[60vh] overflow-y-auto">
    {#each displayedTasks as task (task.id)}
      <TaskCard {task} onclick={() => (selectedTaskId = task.id)} />
    {:else}
      <div class="text-muted-foreground text-center py-15 px-10">
        {filter === "active" ? "No active tasks." : "No completed tasks."}
      </div>
    {/each}
  </div>
</div>

{#if selectedTaskId}
  <TaskDetail taskId={selectedTaskId} onClose={() => (selectedTaskId = null)} />
{/if}
