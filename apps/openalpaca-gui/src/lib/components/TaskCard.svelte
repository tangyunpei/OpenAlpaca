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

  function statusBadgeClass(status: string): string {
    const key = getStatusColor(status);
    switch (key) {
      case "status-success":
        return "bg-success/20 text-success";
      case "status-warning":
        return "bg-amber-400/20 text-amber-400";
      case "status-paused":
        return "bg-blue-400/20 text-blue-400";
      case "status-error":
        return "bg-danger/20 text-danger";
      case "status-dim":
      default:
        return "bg-white/5 text-muted-foreground";
    }
  }
</script>

<button
  class="block w-full text-left px-5 py-4 bg-white/[0.03] rounded-[10px] mb-2.5 border border-white/5 transition-all duration-200 cursor-pointer text-foreground font-[inherit] text-[inherit] hover:bg-white/[0.06] hover:translate-x-1 hover:border-primary hover:shadow-md"
  {onclick}
>
  <div class="flex justify-between items-center mb-2">
    <h3 class="m-0 text-base font-semibold flex-1 overflow-hidden text-ellipsis whitespace-nowrap mr-3">{task.title}</h3>
    <span class="text-[0.7rem] px-2 py-0.5 rounded-full uppercase font-bold shrink-0 {statusBadgeClass(task.status)}">{task.status}</span>
  </div>
  {#if hasProgress}
    <div class="h-1 bg-white/[0.08] rounded-sm overflow-hidden mb-1">
      <div class="h-full bg-accent rounded-sm transition-[width] duration-300 ease-in-out" style="width: {progressPercent}%"></div>
    </div>
    <span class="text-[0.7rem] text-muted-foreground mb-1.5 block">{task.progress_current}/{task.progress_total} <span class="text-accent/80 font-medium">{progressPercent}%</span></span>
  {/if}
  {#if task.assigned_agents && task.assigned_agents.length > 0}
    <div class="flex gap-1.5 flex-wrap mb-1.5">
      {#each task.assigned_agents as agent}
        <span class="text-[0.7rem] px-2 py-px rounded-xl bg-violet-500/20 text-violet-400 font-mono">{agent.agent_id}</span>
      {/each}
    </div>
  {/if}
  <div class="flex gap-3 text-xs text-muted-foreground font-mono">
    <span class="bg-primary px-1.5 rounded text-foreground">{task.id.slice(0, 8)}</span>
    <span>{formatTime(task.created_at)}</span>
  </div>
</button>
