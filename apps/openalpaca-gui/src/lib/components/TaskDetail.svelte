<script lang="ts">
  import { onMount } from "svelte";
  import { loadTaskDetail, selectedTaskDetail } from "$lib/stores/tasks";
  import { performTaskAction } from "$lib/api/tasks";
  import { formatDateTime, getStatusColor } from "$lib/utils";
  import type { TaskDetailResponse } from "$lib/types";

  interface Props {
    taskId: string;
    onClose: () => void;
  }

  let { taskId, onClose }: Props = $props();

  let detail = $state<TaskDetailResponse | null>(null);
  let loading = $state(true);
  let actionLoading = $state(false);

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
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div class="modal" onclick={(e) => e.stopPropagation()} role="dialog">
    {#if loading}
      <div class="loading">Loading...</div>
    {:else if detail}
      <div class="modal-header">
        <h3>{detail.task.title}</h3>
        <button class="close-btn" onclick={onClose}>×</button>
      </div>

      <div class="detail-grid">
        <div class="detail-row">
          <span class="label">Status</span>
          <span class="status-badge {getStatusColor(detail.task.status)}">{detail.task.status}</span>
        </div>
        <div class="detail-row">
          <span class="label">ID</span>
          <span class="mono">{detail.task.id}</span>
        </div>
        <div class="detail-row">
          <span class="label">Priority</span>
          <span>{detail.task.priority}</span>
        </div>
        <div class="detail-row">
          <span class="label">Created</span>
          <span>{formatDateTime(detail.task.created_at)}</span>
        </div>
        <div class="detail-row">
          <span class="label">Updated</span>
          <span>{formatDateTime(detail.task.updated_at)}</span>
        </div>
        {#if detail.task.completed_at}
          <div class="detail-row">
            <span class="label">Completed</span>
            <span>{formatDateTime(detail.task.completed_at)}</span>
          </div>
        {/if}
        <div class="detail-row">
          <span class="label">Created by</span>
          <span>{detail.task.created_by}</span>
        </div>
        <div class="detail-row">
          <span class="label">Source lane</span>
          <span>{detail.task.source_lane}</span>
        </div>
        {#if detail.task.progress_current != null && detail.task.progress_total != null}
          <div class="detail-row">
            <span class="label">Progress</span>
            <span>{detail.task.progress_current} / {detail.task.progress_total}</span>
          </div>
        {/if}
      </div>

      {#if detail.task.description}
        <div class="section">
          <h4>Description</h4>
          <p class="description">{detail.task.description}</p>
        </div>
      {/if}

      {#if detail.task.result_summary}
        <div class="section">
          <h4>Result</h4>
          <p class="description">{detail.task.result_summary}</p>
        </div>
      {/if}

      {#if detail.assignments && detail.assignments.length > 0}
        <div class="section">
          <h4>Assignments</h4>
          <table class="assignments-table">
            <thead>
              <tr>
                <th>Agent</th>
                <th>Role</th>
                <th>Status</th>
                <th>Order</th>
              </tr>
            </thead>
            <tbody>
              {#each detail.assignments as a}
                <tr>
                  <td class="mono">{a.agent_id.slice(0, 8)}</td>
                  <td>{a.role}</td>
                  <td><span class="status-badge small {getStatusColor(a.status)}">{a.status}</span></td>
                  <td>{a.step_order ?? "-"}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}

      <div class="modal-actions">
        {#if canCancel}
          <button class="danger" onclick={() => handleAction("cancel")} disabled={actionLoading}>Cancel</button>
        {/if}
        {#if canPause}
          <button onclick={() => handleAction("pause")} disabled={actionLoading}>Pause</button>
        {/if}
        {#if canResume}
          <button onclick={() => handleAction("resume")} disabled={actionLoading}>Resume</button>
        {/if}
        <button class="secondary" onclick={onClose}>Close</button>
      </div>
    {:else}
      <div class="loading">Task not found.</div>
    {/if}
  </div>
</div>

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

  .modal-header h3 {
    margin: 0;
    font-size: 1.2rem;
    color: var(--text);
    flex: 1;
    margin-right: 12px;
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

  .detail-row {
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

  .description {
    margin: 0;
    font-size: 0.9rem;
    color: var(--text);
    background: rgba(0, 0, 0, 0.2);
    padding: 10px 14px;
    border-radius: 8px;
    line-height: 1.5;
  }

  .assignments-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.85rem;
  }

  .assignments-table th {
    text-align: left;
    color: var(--text-dim);
    font-weight: 500;
    padding: 6px 8px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  }

  .assignments-table td {
    padding: 6px 8px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.04);
  }

  .status-badge {
    font-size: 0.7rem;
    padding: 2px 8px;
    border-radius: 20px;
    text-transform: uppercase;
    font-weight: 700;
  }

  .status-badge.small {
    font-size: 0.65rem;
    padding: 1px 6px;
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    margin-top: 16px;
    padding-top: 16px;
    border-top: 1px solid rgba(255, 255, 255, 0.05);
  }

  .loading {
    text-align: center;
    color: var(--text-dim);
    padding: 40px;
  }
</style>
