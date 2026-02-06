<script lang="ts">
  import type { Agent, Skill } from "$lib/types";
  import { getStatusColor, safeParseJson, resolveIcon } from "$lib/utils";

  interface Props {
    agent: Agent;
    onclick: () => void;
  }

  let { agent, onclick }: Props = $props();

  let skills = $derived(safeParseJson<Skill[]>(agent.skills_json, []));
</script>

<button class="agent-card" {onclick}>
  <div class="agent-header">
    <div class="agent-icon">
      <span class="icon-text">{resolveIcon(agent.icon)}</span>
    </div>
    <div class="agent-info">
      <h3 class="agent-name">{agent.name}</h3>
      {#if agent.description}
        <p class="agent-desc">{agent.description}</p>
      {/if}
    </div>
    <span class="status-dot {getStatusColor(agent.status)}" title={agent.status}></span>
  </div>
  {#if agent.current_task_id}
    <div class="current-task">
      <span class="label">Task:</span>
      <span class="mono">{agent.current_task_id.slice(0, 8)}</span>
    </div>
  {/if}
  {#if skills.length > 0}
    <div class="skill-tags">
      {#each skills.slice(0, 4) as skill}
        <span class="skill-tag">{skill.name}</span>
      {/each}
      {#if skills.length > 4}
        <span class="skill-tag more">+{skills.length - 4}</span>
      {/if}
    </div>
  {/if}
</button>

<style>
  .agent-card {
    display: block;
    width: 100%;
    text-align: left;
    padding: 16px 20px;
    background: rgba(255, 255, 255, 0.03);
    border-radius: 10px;
    margin-bottom: 10px;
    border: 1px solid rgba(255, 255, 255, 0.05);
    transition: all 0.2s;
    cursor: pointer;
    color: var(--text);
    font-family: inherit;
    font-size: inherit;
  }

  .agent-card:hover {
    background: rgba(255, 255, 255, 0.06);
    transform: translateX(4px);
    border-color: var(--primary);
  }

  .agent-header {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 8px;
  }

  .agent-icon {
    width: 36px;
    height: 36px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(255, 255, 255, 0.05);
    border-radius: 8px;
    flex-shrink: 0;
    color: var(--text-dim);
  }

  .icon-text {
    font-size: 1.3rem;
  }

  .agent-info {
    flex: 1;
    min-width: 0;
  }

  .agent-name {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .agent-desc {
    margin: 2px 0 0;
    font-size: 0.8rem;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .status-dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .status-dot.status-success {
    background: var(--success);
  }
  .status-dot.status-warning {
    background: #fbbf24;
  }
  .status-dot.status-paused {
    background: #60a5fa;
  }
  .status-dot.status-error {
    background: var(--error);
  }
  .status-dot.status-dim {
    background: var(--text-dim);
  }

  .current-task {
    font-size: 0.75rem;
    color: var(--text-dim);
    margin-bottom: 8px;
  }

  .current-task .label {
    margin-right: 4px;
  }

  .mono {
    font-family: "Fira Code", monospace;
    background: var(--primary);
    padding: 0 4px;
    border-radius: 3px;
    color: var(--text);
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

  .skill-tag.more {
    background: rgba(255, 255, 255, 0.05);
    color: var(--text-dim);
  }
</style>
