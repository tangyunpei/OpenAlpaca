<script lang="ts">
  import { onMount } from "svelte";
  import { loadAgentDetail, selectedAgentDetail } from "$lib/stores/agents";
  import { performAgentAction } from "$lib/api/agents";
  import { formatDateTime, formatDuration, getStatusColor, safeParseJson, resolveIcon } from "$lib/utils";
  import type { AgentDetailResponse, Skill } from "$lib/types";
  import AgentConfigEditor from "./AgentConfigEditor.svelte";
  import DeleteAgentDialog from "./DeleteAgentDialog.svelte";

  interface Props {
    agentId: string;
    onClose: () => void;
  }

  let { agentId, onClose }: Props = $props();

  let detail = $state<AgentDetailResponse | null>(null);
  let loading = $state(true);
  let actionLoading = $state(false);
  let showConfigEditor = $state(false);
  let showDeleteDialog = $state(false);

  const unsubDetail = selectedAgentDetail.subscribe((v) => (detail = v));

  onMount(() => {
    loadAgentDetail(agentId).finally(() => (loading = false));
    return () => unsubDetail();
  });

  async function handleAction(action: "pause" | "resume") {
    actionLoading = true;
    try {
      await performAgentAction(agentId, action);
      await loadAgentDetail(agentId);
    } catch (e) {
      alert(`Action failed: ${e}`);
    } finally {
      actionLoading = false;
    }
  }

  function handleDeleted() {
    showDeleteDialog = false;
    onClose();
  }

  let skills = $derived(detail ? safeParseJson<Skill[]>(detail.agent.skills_json, []) : []);
  let llmConfig = $derived(
    detail ? safeParseJson<Record<string, unknown>>(detail.agent.llm_config_json, {}) : {},
  );
  let constraints = $derived(
    detail ? safeParseJson<Record<string, unknown>>(detail.agent.constraints_json, {}) : {},
  );
  let canPause = $derived(detail?.agent.status === "idle" || detail?.agent.status === "busy");
  let canResume = $derived(detail?.agent.status === "waiting");

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
        return "bg-muted-foreground/20 text-muted-foreground";
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="fixed inset-0 bg-black/60 flex justify-center items-center z-[1000]" onclick={onClose} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div
    class="bg-surface p-6 rounded-xl w-[90%] max-w-[560px] max-h-[80vh] overflow-y-auto shadow-[0_4px_20px_rgba(0,0,0,0.3)] border border-border"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
  >
    {#if loading}
      <div class="text-center text-muted-foreground py-10">Loading...</div>
    {:else if detail}
      <!-- Header -->
      <div class="flex justify-between items-start mb-4">
        <div class="flex gap-3 items-center flex-1">
          <div class="w-11 h-11 flex items-center justify-center bg-white/5 rounded-[10px] shrink-0 text-muted-foreground">
            <span class="text-2xl">{resolveIcon(detail.agent.icon)}</span>
          </div>
          <div>
            <h3 class="m-0 text-[1.2rem] text-foreground">{detail.agent.name}</h3>
            {#if detail.agent.description}
              <p class="mt-0.5 mb-0 text-xs text-muted-foreground">{detail.agent.description}</p>
            {/if}
          </div>
        </div>
        <button
          class="bg-transparent border-none text-muted-foreground text-2xl cursor-pointer p-0 leading-none hover:text-foreground"
          onclick={onClose}
        >&times;</button>
      </div>

      <!-- Detail grid -->
      <div class="flex flex-col gap-2 mb-4">
        <div class="flex justify-between items-center text-[0.85rem]">
          <span class="text-muted-foreground font-medium">Status</span>
          <span class="text-[0.7rem] px-2 py-0.5 rounded-[20px] uppercase font-bold {statusBadgeClass(detail.agent.status)}">{detail.agent.status}</span>
        </div>
        <div class="flex justify-between items-center text-[0.85rem]">
          <span class="text-muted-foreground font-medium">ID</span>
          <span class="font-mono text-xs text-muted-foreground">{detail.agent.id}</span>
        </div>
        {#if detail.agent.current_task_id}
          <div class="flex justify-between items-center text-[0.85rem]">
            <span class="text-muted-foreground font-medium">Current Task</span>
            <span class="font-mono text-xs text-muted-foreground">{detail.agent.current_task_id}</span>
          </div>
        {/if}
        <div class="flex justify-between items-center text-[0.85rem]">
          <span class="text-muted-foreground font-medium">Created</span>
          <span>{formatDateTime(detail.agent.created_at)}</span>
        </div>
      </div>

      <!-- Skills -->
      {#if skills.length > 0}
        <div class="mb-4">
          <h4 class="m-0 mb-2 text-[0.85rem] text-muted-foreground uppercase tracking-wide">Skills</h4>
          <div class="flex flex-wrap gap-1.5">
            {#each skills as skill}
              <span class="text-[0.7rem] px-2 py-0.5 rounded-xl bg-accent/12 text-accent font-medium">{skill.name}</span>
            {/each}
          </div>
        </div>
      {/if}

      <!-- Metrics -->
      {#if detail.metrics}
        <div class="mb-4">
          <h4 class="m-0 mb-2 text-[0.85rem] text-muted-foreground uppercase tracking-wide">Metrics</h4>
          <div class="grid grid-cols-[repeat(auto-fit,minmax(100px,1fr))] gap-2.5 mb-2">
            <div class="text-center py-2.5 px-1.5 bg-black/20 rounded-lg">
              <span class="block text-[1.1rem] font-bold text-foreground mb-0.5">{detail.metrics.tasks_completed}</span>
              <span class="text-[0.65rem] text-muted-foreground uppercase">Completed</span>
            </div>
            <div class="text-center py-2.5 px-1.5 bg-black/20 rounded-lg">
              <span class="block text-[1.1rem] font-bold text-foreground mb-0.5">{detail.metrics.tasks_failed}</span>
              <span class="text-[0.65rem] text-muted-foreground uppercase">Failed</span>
            </div>
            <div class="text-center py-2.5 px-1.5 bg-black/20 rounded-lg">
              <span class="block text-[1.1rem] font-bold text-foreground mb-0.5">{Math.round(detail.metrics.success_rate * 100)}%</span>
              <span class="text-[0.65rem] text-muted-foreground uppercase">Success Rate</span>
            </div>
            <div class="text-center py-2.5 px-1.5 bg-black/20 rounded-lg">
              <span class="block text-[1.1rem] font-bold text-foreground mb-0.5">{formatDuration(detail.metrics.average_runtime_seconds)}</span>
              <span class="text-[0.65rem] text-muted-foreground uppercase">Avg Runtime</span>
            </div>
          </div>
          {#if detail.metrics.success_rate >= 0}
            <div class="h-1 bg-white/8 rounded-sm overflow-hidden">
              <div class="h-full bg-success rounded-sm transition-[width] duration-300 ease-out" style="width: {detail.metrics.success_rate * 100}%"></div>
            </div>
          {/if}
        </div>
      {/if}

      <!-- LLM Config -->
      {#if Object.keys(llmConfig).length > 0}
        <div class="mb-4">
          <h4 class="m-0 mb-2 text-[0.85rem] text-muted-foreground uppercase tracking-wide">LLM Config</h4>
          <div class="flex flex-col gap-1.5 bg-black/20 px-3.5 py-2.5 rounded-lg">
            {#if llmConfig.model}
              <div class="flex justify-between items-center text-[0.85rem]">
                <span class="text-muted-foreground font-medium">Model</span>
                <span>{llmConfig.model}</span>
              </div>
            {/if}
            {#if Array.isArray(llmConfig.fallback_models) && llmConfig.fallback_models.length > 0}
              <div class="flex justify-between items-center text-[0.85rem]">
                <span class="text-muted-foreground font-medium">Fallbacks</span>
                <span>{(llmConfig.fallback_models as string[]).join(", ")}</span>
              </div>
            {/if}
          </div>
        </div>
      {/if}

      <!-- Constraints -->
      {#if Object.keys(constraints).length > 0}
        <div class="mb-4">
          <h4 class="m-0 mb-2 text-[0.85rem] text-muted-foreground uppercase tracking-wide">Constraints</h4>
          <div class="flex flex-col gap-1.5 bg-black/20 px-3.5 py-2.5 rounded-lg">
            {#if Array.isArray(constraints.allowed_models) && constraints.allowed_models.length > 0}
              <div class="flex justify-between items-center text-[0.85rem]">
                <span class="text-muted-foreground font-medium">Allowed models</span>
                <span>{(constraints.allowed_models as string[]).join(", ")}</span>
              </div>
            {/if}
            {#if Array.isArray(constraints.denied_models) && constraints.denied_models.length > 0}
              <div class="flex justify-between items-center text-[0.85rem]">
                <span class="text-muted-foreground font-medium">Denied models</span>
                <span>{(constraints.denied_models as string[]).join(", ")}</span>
              </div>
            {/if}
            {#if constraints.max_cost_per_task != null}
              <div class="flex justify-between items-center text-[0.85rem]">
                <span class="text-muted-foreground font-medium">Max cost/task</span>
                <span>${constraints.max_cost_per_task}</span>
              </div>
            {/if}
          </div>
        </div>
      {/if}

      <!-- Actions -->
      <div class="flex justify-end gap-2.5 mt-4 pt-4 border-t border-border">
        <button
          class="px-4 py-2 text-sm bg-white/5 text-foreground border border-input rounded-lg cursor-pointer hover:bg-white/10 transition-colors"
          onclick={() => (showConfigEditor = true)}
        >Edit Config</button>
        {#if canPause}
          <button
            class="px-4 py-2 text-sm bg-white/5 text-foreground border border-input rounded-lg cursor-pointer hover:bg-white/10 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            onclick={() => handleAction("pause")}
            disabled={actionLoading}
          >Pause</button>
        {/if}
        {#if canResume}
          <button
            class="px-4 py-2 text-sm bg-white/5 text-foreground border border-input rounded-lg cursor-pointer hover:bg-white/10 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            onclick={() => handleAction("resume")}
            disabled={actionLoading}
          >Resume</button>
        {/if}
        <button
          class="px-4 py-2 text-sm bg-transparent text-danger border border-danger rounded-lg cursor-pointer hover:bg-danger/15 transition-colors"
          onclick={() => (showDeleteDialog = true)}
        >Delete</button>
        <button
          class="px-4 py-2 text-sm bg-white/5 text-muted-foreground border border-input rounded-lg cursor-pointer hover:bg-white/10 transition-colors"
          onclick={onClose}
        >Close</button>
      </div>
    {:else}
      <div class="text-center text-muted-foreground py-10">Agent not found.</div>
    {/if}
  </div>
</div>

{#if showConfigEditor}
  <AgentConfigEditor agentId={agentId} onClose={() => { showConfigEditor = false; loadAgentDetail(agentId); }} />
{/if}

{#if showDeleteDialog && detail}
  <DeleteAgentDialog
    agentId={agentId}
    agentName={detail.agent.name}
    onClose={() => (showDeleteDialog = false)}
    onDeleted={handleDeleted}
  />
{/if}
