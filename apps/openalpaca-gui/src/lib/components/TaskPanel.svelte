<script lang="ts">
  import { activeTasks, completedTasks, loadTasks, tasksLoading } from "$lib/stores/tasks";
  import type { Task } from "$lib/types";
  import TaskCard from "./TaskCard.svelte";
  import TaskDetail from "./TaskDetail.svelte";

  let filter = $state<"active" | "completed">("active");
  let loading = $state(false);
  let displayedTasks = $state<Task[]>([]);
  let selectedTaskId = $state<string | null>(null);

  // Counts for filter badges
  let activeCount = $state(0);
  let completedCount = $state(0);

  $effect(() => {
    const unsub = activeTasks.subscribe((v) => (activeCount = v.length));
    return unsub;
  });
  $effect(() => {
    const unsub = completedTasks.subscribe((v) => (completedCount = v.length));
    return unsub;
  });

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
    >Active{#if activeCount > 0}<span class="ml-1 text-[0.65rem] opacity-70">({activeCount})</span>{/if}</button>
    <button
      class="px-4 py-1.5 border-none bg-transparent text-muted-foreground cursor-pointer text-[0.8rem] font-medium rounded-[5px] transition-all duration-200 hover:text-foreground {filter === 'completed' ? 'bg-white/10 text-white' : ''}"
      onclick={() => setFilter("completed")}
    >Completed{#if completedCount > 0}<span class="ml-1 text-[0.65rem] opacity-70">({completedCount})</span>{/if}</button>
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
      <div class="flex flex-col items-center justify-center py-14 px-10 text-center">
        <div class="w-12 h-12 mb-3 rounded-xl bg-white/[0.03] border border-white/5 flex items-center justify-center">
          <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class="text-muted-foreground/50">
            {#if filter === "active"}
              <path d="M12 2v4m0 12v4M4.93 4.93l2.83 2.83m8.48 8.48 2.83 2.83M2 12h4m12 0h4M4.93 19.07l2.83-2.83m8.48-8.48 2.83-2.83"/>
            {:else}
              <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/><polyline points="22 4 12 14.01 9 11.01"/>
            {/if}
          </svg>
        </div>
        <p class="text-muted-foreground text-sm m-0">{filter === "active" ? "No active tasks" : "No completed tasks"}</p>
        <p class="text-muted-foreground/50 text-xs m-0 mt-1">{filter === "active" ? "Tasks will appear here when created" : "Completed tasks will be listed here"}</p>
      </div>
    {/each}
  </div>
</div>

{#if selectedTaskId}
  <TaskDetail taskId={selectedTaskId} onClose={() => (selectedTaskId = null)} />
{/if}
