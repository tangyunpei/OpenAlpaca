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
        return "bg-success/15 text-success border-success/20";
      case "status-warning":
        return "bg-amber-400/15 text-amber-400 border-amber-400/20";
      case "status-paused":
        return "bg-blue-400/15 text-blue-400 border-blue-400/20";
      case "status-error":
        return "bg-danger/15 text-danger border-danger/20";
      case "status-dim":
      default:
        return "bg-white/[0.04] text-muted-foreground border-white/5";
    }
  }
</script>

<button
  class="group block w-full text-left px-5 py-4 rounded-xl mb-2.5 border border-white/[0.05] transition-all duration-250 cursor-pointer text-foreground font-[inherit] text-[inherit]
         hover:border-white/10 hover:translate-x-0.5 hover:shadow-lg"
  style="background: linear-gradient(135deg, rgba(255,255,255,0.025) 0%, rgba(255,255,255,0.01) 100%);"
  onmouseenter={(e) => { e.currentTarget.style.background = 'linear-gradient(135deg, rgba(255,255,255,0.045) 0%, rgba(255,255,255,0.02) 100%)'; }}
  onmouseleave={(e) => { e.currentTarget.style.background = 'linear-gradient(135deg, rgba(255,255,255,0.025) 0%, rgba(255,255,255,0.01) 100%)'; }}
  {onclick}
>
  <div class="flex justify-between items-start mb-2.5 gap-3">
    <h3 class="m-0 text-[0.92rem] font-semibold flex-1 overflow-hidden text-ellipsis whitespace-nowrap leading-snug" style="letter-spacing: -0.01em;">{task.title}</h3>
    <span class="text-[0.65rem] px-2 py-0.5 rounded-lg uppercase font-bold shrink-0 border {statusBadgeClass(task.status)}" style="letter-spacing: 0.04em;">{task.status}</span>
  </div>
  {#if hasProgress}
    <div class="h-1.5 rounded-full overflow-hidden mb-1.5" style="background: rgba(255,255,255,0.06);">
      <div class="h-full rounded-full transition-[width] duration-500 ease-out"
           style="width: {progressPercent}%; background: linear-gradient(90deg, hsl(40 85% 58%) 0%, hsl(40 85% 65%) 100%);"></div>
    </div>
    <span class="text-[0.68rem] text-muted-foreground mb-2 block">{task.progress_current}/{task.progress_total} <span class="text-accent font-semibold">{progressPercent}%</span></span>
  {/if}
  {#if task.assigned_agents && task.assigned_agents.length > 0}
    <div class="flex gap-1.5 flex-wrap mb-2">
      {#each task.assigned_agents as agent}
        <span class="text-[0.65rem] px-2 py-0.5 rounded-lg font-mono border border-violet-500/15"
              style="background: linear-gradient(135deg, rgba(139, 92, 246, 0.1) 0%, rgba(139, 92, 246, 0.05) 100%); color: rgb(167, 139, 250);">{agent.agent_id}</span>
      {/each}
    </div>
  {/if}
  <div class="flex gap-2.5 items-center text-[0.68rem] text-muted-foreground/60 font-mono">
    <span class="px-1.5 py-px rounded-md text-foreground/70" style="background: linear-gradient(135deg, hsl(222 48% 22%) 0%, hsl(222 48% 18%) 100%);">{task.id.slice(0, 8)}</span>
    <span>{formatTime(task.created_at)}</span>
  </div>
</button>
