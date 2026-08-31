<script lang="ts">
  import type { Task } from "$lib/types";
  import type { WorkflowActivityEntry } from "$lib/stores/workflow";
  import { formatTime, getStatusColor } from "$lib/utils";

  interface Props {
    task: Task;
    onclick: () => void;
    workflowEntries?: WorkflowActivityEntry[];
  }

  let { task, onclick, workflowEntries = [] }: Props = $props();

  let hasProgress = $derived(
    task.progress_current != null && task.progress_total != null && task.progress_total > 0,
  );
  let isActive = $derived(
    task.status === "queued" || task.status === "running" || task.status === "paused",
  );
  let latestProgress = $derived(
    isActive ? (workflowEntries.find((e) => e.kind === "progress") ?? null) : null,
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
    {#if (task.status === "completed" || task.status === "failed") && task.outcome_kind && task.outcome_kind !== 'failed'}
      <span class="text-[0.6rem] px-1.5 py-0.5 rounded-lg font-medium shrink-0 border
        {task.outcome_kind === 'text_only'
          ? 'bg-blue-400/10 text-blue-400/80 border-blue-400/15'
          : task.outcome_kind === 'artifact_only' || task.outcome_kind === 'mixed'
            ? 'bg-emerald-400/10 text-emerald-400/80 border-emerald-400/15'
            : 'bg-white/[0.04] text-muted-foreground border-white/5'}">
        {#if task.outcome_kind === 'text_only'}
          Text only
        {:else if task.outcome_kind === 'artifact_only'}
          {task.artifact_count ?? 0} file{(task.artifact_count ?? 0) !== 1 ? 's' : ''}
        {:else if task.outcome_kind === 'mixed'}
          {task.artifact_count ?? 0} file{(task.artifact_count ?? 0) !== 1 ? 's' : ''} + text
        {/if}
      </span>
    {/if}
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
  {#if latestProgress}
    <p class="flex items-center gap-1.5 text-[0.72rem] text-muted-foreground/80 mt-1 mb-2 leading-relaxed">
      <span class="w-1.5 h-1.5 rounded-full bg-accent animate-pulse shrink-0"></span>
      <span class="overflow-hidden text-ellipsis whitespace-nowrap flex-1">{latestProgress.message}</span>
      <span class="font-mono text-[0.62rem] text-muted-foreground/50 shrink-0">{formatTime(latestProgress.ts)}</span>
    </p>
  {/if}
  {#if task.result_summary && (task.status === "completed" || task.status === "failed")}
    <p class="text-[0.75rem] text-muted-foreground/70 mt-1.5 mb-0 line-clamp-2 leading-relaxed">{task.result_summary}</p>
  {/if}
  <div class="flex gap-2.5 items-center text-[0.68rem] text-muted-foreground/60 font-mono">
    <span class="px-1.5 py-px rounded-md text-foreground/70" style="background: linear-gradient(135deg, hsl(222 48% 22%) 0%, hsl(222 48% 18%) 100%);">{task.id.slice(0, 8)}</span>
    <span>{formatTime(task.created_at)}</span>
  </div>
</button>
