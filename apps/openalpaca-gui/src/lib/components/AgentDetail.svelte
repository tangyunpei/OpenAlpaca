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
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div class="modal" onclick={(e) => e.stopPropagation()} role="dialog">
    {#if loading}
      <div class="loading">Loading...</div>
    {:else if detail}
      <div class="modal-header">
        <div class="agent-header-info">
          <div class="agent-icon">
            <span class="icon-text">{resolveIcon(detail.agent.icon)}</span>
          </div>
          <div>
            <h3>{detail.agent.name}</h3>
            {#if detail.agent.description}
              <p class="agent-desc">{detail.agent.description}</p>
            {/if}
          </div>
        </div>
        <button class="close-btn" onclick={onClose}>×</button>
      </div>

      <div class="detail-grid">
        <div class="detail-row">
          <span class="label">Status</span>
          <span class="status-badge {getStatusColor(detail.agent.status)}">{detail.agent.status}</span>
        </div>
        <div class="detail-row">
          <span class="label">ID</span>
          <span class="mono">{detail.agent.id}</span>
        </div>
        {#if detail.agent.current_task_id}
          <div class="detail-row">
            <span class="label">Current Task</span>
            <span class="mono">{detail.agent.current_task_id}</span>
          </div>
        {/if}
        <div class="detail-row">
          <span class="label">Created</span>
          <span>{formatDateTime(detail.agent.created_at)}</span>
        </div>
      </div>

      {#if skills.length > 0}
        <div class="section">
          <h4>Skills</h4>
          <div class="skill-tags">
            {#each skills as skill}
              <span class="skill-tag">{skill.name}</span>
            {/each}
          </div>
        </div>
      {/if}

      {#if detail.metrics}
        <div class="section">
          <h4>Metrics</h4>
          <div class="metrics-grid">
            <div class="metric">
              <span class="metric-value">{detail.metrics.tasks_completed}</span>
              <span class="metric-label">Completed</span>
            </div>
            <div class="metric">
              <span class="metric-value">{detail.metrics.tasks_failed}</span>
              <span class="metric-label">Failed</span>
            </div>
            <div class="metric">
              <span class="metric-value">{Math.round(detail.metrics.success_rate * 100)}%</span>
              <span class="metric-label">Success Rate</span>
            </div>
            <div class="metric">
              <span class="metric-value">{formatDuration(detail.metrics.average_runtime_seconds)}</span>
              <span class="metric-label">Avg Runtime</span>
            </div>
          </div>
          {#if detail.metrics.success_rate >= 0}
            <div class="success-bar">
              <div class="success-fill" style="width: {detail.metrics.success_rate * 100}%"></div>
            </div>
          {/if}
        </div>
      {/if}

      {#if Object.keys(llmConfig).length > 0}
        <div class="section">
          <h4>LLM Config</h4>
          <div class="config-block">
            {#if llmConfig.model}
              <div class="config-row">
                <span class="label">Model</span>
                <span>{llmConfig.model}</span>
              </div>
            {/if}
            {#if Array.isArray(llmConfig.fallback_models) && llmConfig.fallback_models.length > 0}
              <div class="config-row">
                <span class="label">Fallbacks</span>
                <span>{(llmConfig.fallback_models as string[]).join(", ")}</span>
              </div>
            {/if}
          </div>
        </div>
      {/if}

      {#if Object.keys(constraints).length > 0}
        <div class="section">
          <h4>Constraints</h4>
          <div class="config-block">
            {#if Array.isArray(constraints.allowed_models) && constraints.allowed_models.length > 0}
              <div class="config-row">
                <span class="label">Allowed models</span>
                <span>{(constraints.allowed_models as string[]).join(", ")}</span>
              </div>
            {/if}
            {#if Array.isArray(constraints.denied_models) && constraints.denied_models.length > 0}
              <div class="config-row">
                <span class="label">Denied models</span>
                <span>{(constraints.denied_models as string[]).join(", ")}</span>
              </div>
            {/if}
            {#if constraints.max_cost_per_task != null}
              <div class="config-row">
                <span class="label">Max cost/task</span>
                <span>${constraints.max_cost_per_task}</span>
              </div>
            {/if}
          </div>
        </div>
      {/if}

      <div class="modal-actions">
        <button onclick={() => (showConfigEditor = true)}>Edit Config</button>
        {#if canPause}
          <button onclick={() => handleAction("pause")} disabled={actionLoading}>Pause</button>
        {/if}
        {#if canResume}
          <button onclick={() => handleAction("resume")} disabled={actionLoading}>Resume</button>
        {/if}
        <button class="danger-outline" onclick={() => (showDeleteDialog = true)}>Delete</button>
        <button class="secondary" onclick={onClose}>Close</button>
      </div>
    {:else}
      <div class="loading">Agent not found.</div>
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

<style>
  .modal-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    width: 100%;
    height: 100%;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    justify-content: center;
    align-items: center;
    z-index: 1000;
  }

  .modal {
    background: var(--surface);
    padding: 24px;
    border-radius: 12px;
    width: 90%;
    max-width: 560px;
    max-height: 80vh;
    overflow-y: auto;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }

  .modal-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    margin-bottom: 16px;
  }

  .agent-header-info {
    display: flex;
    gap: 12px;
    align-items: center;
    flex: 1;
  }

  .agent-icon {
    width: 44px;
    height: 44px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(255, 255, 255, 0.05);
    border-radius: 10px;
    flex-shrink: 0;
    color: var(--text-dim);
  }

  .icon-text {
    font-size: 1.5rem;
  }

  .modal-header h3 {
    margin: 0;
    font-size: 1.2rem;
    color: var(--text);
  }

  .agent-desc {
    margin: 2px 0 0;
    font-size: 0.8rem;
    color: var(--text-dim);
  }

  .close-btn {
    background: none;
    border: none;
    color: var(--text-dim);
    font-size: 1.5rem;
    cursor: pointer;
    padding: 0;
    line-height: 1;
  }
  .close-btn:hover {
    color: var(--text);
  }

  .detail-grid {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-bottom: 16px;
  }

  .detail-row, .config-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 0.85rem;
  }

  .label {
    color: var(--text-dim);
    font-weight: 500;
  }

  .mono {
    font-family: "Fira Code", monospace;
    font-size: 0.8rem;
    color: var(--text-dim);
  }

  .section {
    margin-bottom: 16px;
  }

  .section h4 {
    margin: 0 0 8px 0;
    font-size: 0.85rem;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .skill-tags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .skill-tag {
    font-size: 0.7rem;
    padding: 2px 8px;
    border-radius: 12px;
    background: rgba(233, 69, 96, 0.12);
    color: var(--accent);
    font-weight: 500;
  }

  .metrics-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(100px, 1fr));
    gap: 10px;
    margin-bottom: 8px;
  }

  .metric {
    text-align: center;
    padding: 10px 6px;
    background: rgba(0, 0, 0, 0.2);
    border-radius: 8px;
  }

  .metric-value {
    display: block;
    font-size: 1.1rem;
    font-weight: 700;
    color: var(--text);
    margin-bottom: 2px;
  }

  .metric-label {
    font-size: 0.65rem;
    color: var(--text-dim);
    text-transform: uppercase;
  }

  .success-bar {
    height: 4px;
    background: rgba(255, 255, 255, 0.08);
    border-radius: 2px;
    overflow: hidden;
  }

  .success-fill {
    height: 100%;
    background: var(--success);
    border-radius: 2px;
    transition: width 0.3s ease;
  }

  .config-block {
    display: flex;
    flex-direction: column;
    gap: 6px;
    background: rgba(0, 0, 0, 0.2);
    padding: 10px 14px;
    border-radius: 8px;
  }

  .status-badge {
    font-size: 0.7rem;
    padding: 2px 8px;
    border-radius: 20px;
    text-transform: uppercase;
    font-weight: 700;
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    margin-top: 16px;
    padding-top: 16px;
    border-top: 1px solid rgba(255, 255, 255, 0.05);
  }

  .danger-outline {
    background: transparent;
    border: 1px solid var(--error);
    color: var(--error);
  }
  .danger-outline:hover {
    background: rgba(239, 68, 68, 0.15);
  }

  .loading {
    text-align: center;
    color: var(--text-dim);
    padding: 40px;
  }
</style>
