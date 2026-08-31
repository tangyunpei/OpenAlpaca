<script lang="ts">
  import { activeTasks, completedTasks, loadTasks, tasksLoading } from "$lib/stores/tasks";
  import { workflowActivity, workflowFeed, type WorkflowActivityEntry } from "$lib/stores/workflow";
  import type { Task } from "$lib/types";
  import { formatTime } from "$lib/utils";
  import TaskCard from "./TaskCard.svelte";
  import TaskDetail from "./TaskDetail.svelte";

  let filter = $state<"active" | "completed">("active");
  let loading = $state(false);
  let displayedTasks = $state<Task[]>([]);
  let selectedTaskId = $state<string | null>(null);
  let activityMap = $state<Map<string, WorkflowActivityEntry[]>>(new Map());
  let feed = $state<WorkflowActivityEntry[]>([]);

  // Lightweight lane/lifecycle entries (progress lines render per task card)
  let lifecycleFeed = $derived(feed.filter((e) => e.kind !== "progress").slice(0, 8));

  function kindLabel(kind: WorkflowActivityEntry["kind"]): string {
    switch (kind) {
      case "started":
        return "started";
      case "steered":
        return "steered";
      case "followup_queued":
        return "follow-up";
      default:
        return kind;
    }
  }

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

  $effect(() => {
    const unsub = workflowActivity.subscribe((v) => (activityMap = v));
    return unsub;
  });

  $effect(() => {
    const unsub = workflowFeed.subscribe((v) => (feed = v));
    return unsub;
  });

  function setFilter(f: "active" | "completed") {
    filter = f;
  }

  async function refresh() {
    await loadTasks();
  }
</script>

<div class="flex flex-wrap gap-3 mb-5 items-center">
  <button
    class="px-4 py-2 rounded-xl border border-white/5 text-sm font-medium text-foreground/80 cursor-pointer transition-all duration-200 hover:border-white/10 disabled:opacity-40 disabled:cursor-not-allowed"
    style="background: linear-gradient(180deg, rgba(255,255,255,0.04) 0%, rgba(255,255,255,0.02) 100%);"
    onclick={refresh}
    disabled={loading}
  >
    {#if loading}
      <span class="flex items-center gap-1.5">
        <span class="w-1.5 h-1.5 rounded-full bg-accent animate-pulse"></span>
        Refreshing
      </span>
    {:else}
      Refresh
    {/if}
  </button>
  <div class="oa-tab-group">
    <button
      class="oa-tab-item"
      data-active={filter === 'active'}
      onclick={() => setFilter("active")}
    >Active{#if activeCount > 0}<span class="oa-count-badge">{activeCount}</span>{/if}</button>
    <button
      class="oa-tab-item"
      data-active={filter === 'completed'}
      onclick={() => setFilter("completed")}
    >Completed{#if completedCount > 0}<span class="oa-count-badge">{completedCount}</span>{/if}</button>
  </div>
</div>

<div class="oa-panel">
  <div class="oa-panel-header flex items-center justify-between">
    <h2>Tasks</h2>
    <span class="text-[0.65rem] text-muted-foreground/60 font-mono">{displayedTasks.length} items</span>
  </div>
  <div class="p-3">
    {#each displayedTasks as task, idx (task.id)}
      <div style="animation: slideUp 0.35s cubic-bezier(0.16, 1, 0.3, 1) both; animation-delay: {idx * 50}ms;">
        <TaskCard {task} workflowEntries={activityMap.get(task.id) ?? []} onclick={() => (selectedTaskId = task.id)} />
      </div>
    {:else}
      <div class="flex flex-col items-center justify-center py-16 px-10 text-center animate-fadeIn">
        <div class="w-14 h-14 mb-4 rounded-2xl flex items-center justify-center border border-white/5"
             style="background: linear-gradient(135deg, rgba(255,255,255,0.03) 0%, rgba(255,255,255,0.01) 100%);">
          <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class="text-muted-foreground/40">
            {#if filter === "active"}
              <path d="M12 2v4m0 12v4M4.93 4.93l2.83 2.83m8.48 8.48 2.83 2.83M2 12h4m12 0h4M4.93 19.07l2.83-2.83m8.48-8.48 2.83-2.83"/>
            {:else}
              <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/><polyline points="22 4 12 14.01 9 11.01"/>
            {/if}
          </svg>
        </div>
        <p class="text-foreground/60 text-sm font-medium m-0">{filter === "active" ? "No active tasks" : "No completed tasks"}</p>
        <p class="text-muted-foreground/40 text-xs m-0 mt-1.5 leading-relaxed">{filter === "active" ? "Tasks will appear here when created" : "Completed tasks will be listed here"}</p>
      </div>
    {/each}
  </div>
</div>

{#if lifecycleFeed.length > 0}
  <div class="oa-panel mt-4">
    <div class="oa-panel-header flex items-center justify-between">
      <h2>Workflow Activity</h2>
      <span class="text-[0.65rem] text-muted-foreground/60 font-mono">{lifecycleFeed.length} recent</span>
    </div>
    <ul class="list-none m-0 p-2.5">
      {#each lifecycleFeed as entry (entry._id)}
        <li class="flex items-center gap-2.5 px-3 py-2 rounded-lg mb-1 bg-white/2 text-[0.75rem] transition-colors hover:bg-white/4">
          <span class="text-[0.6rem] px-1.5 py-0.5 rounded-md uppercase font-bold shrink-0 border
            {entry.kind === 'started'
              ? 'bg-success/15 text-success border-success/20'
              : entry.kind === 'steered'
                ? 'bg-blue-400/15 text-blue-400 border-blue-400/20'
                : 'bg-violet-500/15 text-violet-400 border-violet-500/20'}"
            style="letter-spacing: 0.04em;">{kindLabel(entry.kind)}</span>
          <span class="text-muted-foreground flex-1 overflow-hidden text-ellipsis whitespace-nowrap">{entry.message}</span>
          {#if entry.task_id}
            <span class="px-1.5 py-px rounded-md font-mono text-[0.62rem] text-foreground/70 shrink-0" style="background: linear-gradient(135deg, hsl(222 48% 22%) 0%, hsl(222 48% 18%) 100%);">{entry.task_id.slice(0, 8)}</span>
          {/if}
          <span class="text-muted-foreground/60 font-mono text-[0.62rem] shrink-0">{formatTime(entry.ts)}</span>
        </li>
      {/each}
    </ul>
  </div>
{/if}

{#if selectedTaskId}
  <TaskDetail taskId={selectedTaskId} onClose={() => (selectedTaskId = null)} />
{/if}
