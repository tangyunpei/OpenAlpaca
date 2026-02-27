<script lang="ts">
  import { onMount } from "svelte";
  import { loadTaskDetail, selectedTaskDetail } from "$lib/stores/tasks";
  import { performTaskAction } from "$lib/api/tasks";
  import { formatDateTime, getStatusColor } from "$lib/utils";
  import type { TaskDetailResponse } from "$lib/types";

  interface OutcomeArtifact {
    label?: string;
    key?: string;
    agent_id?: string;
  }

  interface Props {
    taskId: string;
    onClose: () => void;
  }

  let { taskId, onClose }: Props = $props();

  let detail = $state<TaskDetailResponse | null>(null);
  let loading = $state(true);
  let actionLoading = $state(false);
  let expandedAssignment = $state<string | null>(null);

  function toggleAssignment(id: string) {
    expandedAssignment = expandedAssignment === id ? null : id;
  }

  let hasDetailProgress = $derived(
    detail?.task.progress_current != null &&
      detail?.task.progress_total != null &&
      detail.task.progress_total > 0,
  );
  let detailProgressPercent = $derived(
    hasDetailProgress
      ? Math.round((detail!.task.progress_current! / detail!.task.progress_total!) * 100)
      : 0,
  );

  const unsubDetail = selectedTaskDetail.subscribe((v) => (detail = v));

  onMount(() => {
    loadTaskDetail(taskId).finally(() => (loading = false));
    return () => unsubDetail();
  });

  async function handleAction(action: "cancel" | "pause" | "resume") {
    actionLoading = true;
    try {
      await performTaskAction(taskId, action);
      await loadTaskDetail(taskId);
    } catch (e) {
      alert(`Action failed: ${e}`);
    } finally {
      actionLoading = false;
    }
  }

  let canCancel = $derived(
    detail?.task.status === "queued" || detail?.task.status === "running" || detail?.task.status === "paused",
  );
  let canPause = $derived(detail?.task.status === "running");
  let canResume = $derived(detail?.task.status === "paused");

  function outcomeKindClass(kind: string): string {
    switch (kind) {
      case "text_only":
        return "bg-blue-400/20 text-blue-400";
      case "artifact_only":
      case "mixed":
        return "bg-emerald-400/20 text-emerald-400";
      case "failed":
        return "bg-danger/20 text-danger";
      default:
        return "bg-white/5 text-muted-foreground";
    }
  }

  function formatOutcomeKind(kind: string): string {
    switch (kind) {
      case "text_only": return "Text Only";
      case "artifact_only": return "Artifact Only";
      case "mixed": return "Mixed";
      case "failed": return "Failed";
      default: return kind;
    }
  }

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

<div class="fixed inset-0 bg-black/60 flex justify-center items-center z-[1000]" onclick={onClose} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div
    class="bg-surface p-6 rounded-xl w-[90%] max-w-[560px] max-h-[80vh] overflow-y-auto shadow-[0_4px_20px_rgba(0,0,0,0.3)] border border-white/[0.08]"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
  >
    {#if loading}
      <div class="text-center text-muted-foreground py-10">Loading...</div>
    {:else if detail}
      <div class="flex justify-between items-start mb-4">
        <h3 class="m-0 text-xl text-foreground flex-1 mr-3">{detail.task.title}</h3>
        <button
          class="bg-transparent border-none text-muted-foreground text-2xl cursor-pointer p-0 leading-none hover:text-foreground"
          onclick={onClose}
        >&times;</button>
      </div>

      <div class="flex flex-col gap-2 mb-4">
        <div class="flex justify-between items-center text-[0.85rem]">
          <span class="text-muted-foreground font-medium">Status</span>
          <span class="text-[0.7rem] px-2 py-0.5 rounded-full uppercase font-bold {statusBadgeClass(detail.task.status)}">{detail.task.status}</span>
        </div>
        <div class="flex justify-between items-center text-[0.85rem]">
          <span class="text-muted-foreground font-medium">ID</span>
          <span class="font-mono text-[0.8rem] text-muted-foreground">{detail.task.id}</span>
        </div>
        <div class="flex justify-between items-center text-[0.85rem]">
          <span class="text-muted-foreground font-medium">Priority</span>
          <span>{detail.task.priority}</span>
        </div>
        <div class="flex justify-between items-center text-[0.85rem]">
          <span class="text-muted-foreground font-medium">Created</span>
          <span>{formatDateTime(detail.task.created_at)}</span>
        </div>
        <div class="flex justify-between items-center text-[0.85rem]">
          <span class="text-muted-foreground font-medium">Updated</span>
          <span>{formatDateTime(detail.task.updated_at)}</span>
        </div>
        {#if detail.task.completed_at}
          <div class="flex justify-between items-center text-[0.85rem]">
            <span class="text-muted-foreground font-medium">Completed</span>
            <span>{formatDateTime(detail.task.completed_at)}</span>
          </div>
        {/if}
        <div class="flex justify-between items-center text-[0.85rem]">
          <span class="text-muted-foreground font-medium">Created by</span>
          <span>{detail.task.created_by}</span>
        </div>
        <div class="flex justify-between items-center text-[0.85rem]">
          <span class="text-muted-foreground font-medium">Source lane</span>
          <span>{detail.task.source_lane}</span>
        </div>
        {#if hasDetailProgress}
          <div class="flex justify-between items-center text-[0.85rem]">
            <span class="text-muted-foreground font-medium">Progress</span>
            <div class="flex items-center gap-2 flex-1 max-w-[200px]">
              <div class="flex-1 h-1 bg-white/[0.08] rounded-sm overflow-hidden">
                <div class="h-full bg-accent rounded-sm transition-[width] duration-300 ease-in-out" style="width: {detailProgressPercent}%"></div>
              </div>
              <span class="text-xs text-muted-foreground whitespace-nowrap">{detail.task.progress_current}/{detail.task.progress_total}</span>
            </div>
          </div>
        {/if}
      </div>

      {#if detail.task.description}
        <div class="mb-4">
          <h4 class="m-0 mb-2 text-[0.85rem] text-muted-foreground uppercase tracking-[0.5px]">Description</h4>
          <p class="m-0 text-[0.9rem] text-foreground bg-black/20 px-3.5 py-2.5 rounded-lg leading-relaxed">{detail.task.description}</p>
        </div>
      {/if}

      {#if detail.task.result_summary}
        <div class="mb-4">
          <h4 class="m-0 mb-2 text-[0.85rem] text-muted-foreground uppercase tracking-[0.5px]">Result</h4>
          <p class="m-0 text-[0.9rem] text-foreground bg-black/20 px-3.5 py-2.5 rounded-lg leading-relaxed">{detail.task.result_summary}</p>
        </div>
      {/if}

      {#if detail.outcome || detail.task.outcome_kind}
        <div class="mb-4">
          <h4 class="m-0 mb-2 text-[0.85rem] text-muted-foreground uppercase tracking-[0.5px]">Outcome</h4>
          <div class="bg-black/20 px-3.5 py-2.5 rounded-lg">
            <!-- Outcome kind badge -->
            <div class="flex items-center gap-2 mb-2">
              <span class="text-[0.7rem] px-2 py-0.5 rounded-full font-bold uppercase
                {outcomeKindClass(detail.outcome?.outcome_kind ?? detail.task.outcome_kind ?? '')}">
                {formatOutcomeKind(detail.outcome?.outcome_kind ?? detail.task.outcome_kind ?? 'unknown')}
              </span>
              {#if (detail.outcome?.artifact_count ?? detail.task.artifact_count ?? 0) > 0}
                <span class="text-[0.8rem] text-muted-foreground">
                  {detail.outcome?.artifact_count ?? detail.task.artifact_count} artifact{(detail.outcome?.artifact_count ?? detail.task.artifact_count ?? 0) !== 1 ? 's' : ''}
                </span>
              {/if}
            </div>

            <!-- Outcome summary -->
            {#if detail.outcome?.outcome_summary}
              <p class="m-0 text-[0.9rem] text-foreground leading-relaxed">{detail.outcome.outcome_summary}</p>
            {/if}

            <!-- No artifact reason -->
            {#if detail.outcome?.no_artifact_reason}
              <p class="m-0 mt-1.5 text-[0.8rem] text-muted-foreground/70 italic">{detail.outcome.no_artifact_reason}</p>
            {/if}

            <!-- Artifact list -->
            {#if detail.outcome?.artifacts && detail.outcome.artifacts.length > 0}
              <div class="mt-2 pt-2 border-t border-white/[0.06]">
                <span class="text-[0.7rem] text-muted-foreground/50 uppercase tracking-wider font-medium">Artifacts</span>
                <div class="flex flex-col gap-1 mt-1">
                  {#each detail.outcome.artifacts as rawArt}
                    {@const art = rawArt as OutcomeArtifact}
                    <div class="flex items-center gap-2 text-[0.8rem]">
                      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                        stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
                        class="shrink-0 text-muted-foreground/50">
                        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8Z"/>
                        <path d="M14 2v6h6"/>
                      </svg>
                      <span class="text-foreground/80">{art.label ?? art.key ?? 'Unnamed'}</span>
                      {#if art.agent_id}
                        <span class="text-muted-foreground/40 text-[0.7rem] font-mono">{art.agent_id}</span>
                      {/if}
                    </div>
                  {/each}
                </div>
              </div>
            {/if}
          </div>
        </div>
      {/if}

      {#if detail.assignments && detail.assignments.length > 0}
        <div class="mb-4">
          <h4 class="m-0 mb-2 text-[0.85rem] text-muted-foreground uppercase tracking-[0.5px]">Assignments</h4>
          <table class="w-full border-collapse text-[0.85rem]">
            <thead>
              <tr>
                <th class="text-left text-muted-foreground font-medium px-2 py-1.5 border-b border-white/[0.08]">Step</th>
                <th class="text-left text-muted-foreground font-medium px-2 py-1.5 border-b border-white/[0.08]">Agent</th>
                <th class="text-left text-muted-foreground font-medium px-2 py-1.5 border-b border-white/[0.08]">Role</th>
                <th class="text-left text-muted-foreground font-medium px-2 py-1.5 border-b border-white/[0.08]">Status</th>
                <th class="text-left text-muted-foreground font-medium px-2 py-1.5 border-b border-white/[0.08]">Output</th>
              </tr>
            </thead>
            <tbody>
              {#each detail.assignments as a}
                <tr>
                  <td class="px-2 py-1.5 border-b border-white/[0.04]">{a.step_order ?? "-"}</td>
                  <td class="px-2 py-1.5 border-b border-white/[0.04] font-mono text-[0.8rem]">{a.agent_id.slice(0, 8)}</td>
                  <td class="px-2 py-1.5 border-b border-white/[0.04]">{a.role}</td>
                  <td class="px-2 py-1.5 border-b border-white/[0.04]">
                    <span class="text-[0.65rem] px-1.5 py-px rounded-full uppercase font-bold {statusBadgeClass(a.status)}">{a.status}</span>
                  </td>
                  <td class="px-2 py-1.5 border-b border-white/[0.04]">
                    {#if a.result_output?.trim()}
                      <button
                        class="bg-violet-500/20 border-none text-violet-400 text-[0.7rem] px-2 py-px rounded-lg cursor-pointer hover:bg-violet-500/35"
                        onclick={() => toggleAssignment(a.id)}
                        aria-expanded={expandedAssignment === a.id}
                      >
                        {expandedAssignment === a.id ? "Hide" : "View"}
                      </button>
                    {:else}
                      <span class="text-muted-foreground">-</span>
                    {/if}
                  </td>
                </tr>
                {#if expandedAssignment === a.id && a.result_output?.trim()}
                  <tr>
                    <td colspan="5" class="px-2 pb-2 border-b border-white/[0.04]">
                      <pre class="m-0 px-3.5 py-2.5 bg-black/25 rounded-lg text-[0.8rem] text-foreground whitespace-pre-wrap break-words max-h-[200px] overflow-y-auto leading-relaxed">{a.result_output}</pre>
                    </td>
                  </tr>
                {/if}
              {/each}
            </tbody>
          </table>
        </div>
      {/if}

      <div class="flex justify-end gap-2.5 mt-4 pt-4 border-t border-white/5">
        {#if canCancel}
          <button
            class="px-4 py-2 rounded-lg border border-danger bg-danger/20 text-danger text-sm font-medium cursor-pointer transition-all duration-200 hover:bg-danger/30 disabled:opacity-50 disabled:cursor-not-allowed"
            onclick={() => handleAction("cancel")}
            disabled={actionLoading}
          >Cancel</button>
        {/if}
        {#if canPause}
          <button
            class="px-4 py-2 rounded-lg border border-border bg-card text-sm font-medium text-foreground cursor-pointer transition-all duration-200 hover:bg-white/5 disabled:opacity-50 disabled:cursor-not-allowed"
            onclick={() => handleAction("pause")}
            disabled={actionLoading}
          >Pause</button>
        {/if}
        {#if canResume}
          <button
            class="px-4 py-2 rounded-lg border border-border bg-card text-sm font-medium text-foreground cursor-pointer transition-all duration-200 hover:bg-white/5 disabled:opacity-50 disabled:cursor-not-allowed"
            onclick={() => handleAction("resume")}
            disabled={actionLoading}
          >Resume</button>
        {/if}
        <button
          class="px-4 py-2 rounded-lg border border-border bg-white/5 text-sm font-medium text-muted-foreground cursor-pointer transition-all duration-200 hover:bg-white/10 hover:text-foreground"
          onclick={onClose}
        >Close</button>
      </div>
    {:else}
      <div class="text-center text-muted-foreground py-10">Task not found.</div>
    {/if}
  </div>
</div>
