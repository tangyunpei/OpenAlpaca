<script lang="ts">
  import { onMount } from "svelte";
  import { loadAgentConfig, saveAgentConfig, selectedAgentConfig } from "$lib/stores/agents";
  import { availableModels, loadAvailableModels } from "$lib/stores/settings";
  import type { AgentConfigFile, AgentConfigResponse, ModelEntry } from "$lib/types";

  interface Props {
    agentId: string;
    onClose: () => void;
  }

  let { agentId, onClose }: Props = $props();

  let configData = $state<AgentConfigResponse | null>(null);
  let loading = $state(true);
  let saving = $state(false);
  let conflict = $state(false);
  let error = $state<string | null>(null);

  let models = $state<ModelEntry[]>([]);

  // Form fields
  let name = $state("");
  let description = $state("");
  let icon = $state("");
  let skillsText = $state("");
  let deniedText = $state("");
  let persona = $state("");
  let temperature = $state(0.5);
  let verbosity = $state("normal");
  let maxToolCalls = $state<string>("");
  let timeoutSeconds = $state<string>("");
  let maxCostPerTask = $state<string>("");
  let confirmFor = $state("");
  let llmModel = $state("");
  let fallbackSet = $state<Set<string>>(new Set());

  const unsubConfig = selectedAgentConfig.subscribe((v) => {
    configData = v;
    if (v) populateForm(v);
  });

  function populateForm(data: AgentConfigResponse) {
    const c = data.config;
    name = c.agent.name;
    description = c.agent.description;
    icon = c.agent.icon || "";
    skillsText = c.skills.assigned.join(", ");
    deniedText = (c.skills.denied || []).join(", ");
    persona = c.preset.persona;
    temperature = c.preset.temperature ?? 0.5;
    verbosity = c.preset.verbosity || "normal";
    maxToolCalls = c.constraints?.max_tool_calls?.toString() || "";
    timeoutSeconds = c.constraints?.timeout_seconds?.toString() || "";
    maxCostPerTask = c.constraints?.max_cost_per_task?.toString() || "";
    confirmFor = (c.constraints?.require_confirmation_for || []).join(", ");
    llmModel = c.llm?.model || "";
    fallbackSet = new Set(c.llm?.fallback_models || []);
  }

  const unsubModels = availableModels.subscribe((v) => (models = v));

  onMount(() => {
    loadAgentConfig(agentId).finally(() => (loading = false));
    loadAvailableModels();
    return () => {
      unsubConfig();
      unsubModels();
    };
  });

  function toggleFallback(modelId: string) {
    const next = new Set(fallbackSet);
    if (next.has(modelId)) {
      next.delete(modelId);
    } else {
      next.add(modelId);
    }
    fallbackSet = next;
  }

  function buildConfig(): AgentConfigFile {
    const assigned = skillsText.split(",").map((s) => s.trim()).filter(Boolean);
    const denied = deniedText.split(",").map((s) => s.trim()).filter(Boolean);

    return {
      agent: {
        id: agentId,
        name,
        description,
        ...(icon ? { icon } : {}),
      },
      skills: {
        assigned,
        ...(denied.length > 0 ? { denied } : {}),
      },
      preset: {
        persona,
        temperature,
        verbosity,
      },
      ...(maxToolCalls || timeoutSeconds || maxCostPerTask || confirmFor
        ? {
            constraints: {
              ...(maxToolCalls ? { max_tool_calls: parseInt(maxToolCalls) } : {}),
              ...(timeoutSeconds ? { timeout_seconds: parseInt(timeoutSeconds) } : {}),
              ...(maxCostPerTask ? { max_cost_per_task: parseFloat(maxCostPerTask) } : {}),
              ...(confirmFor
                ? {
                    require_confirmation_for: confirmFor
                      .split(",")
                      .map((s) => s.trim())
                      .filter(Boolean),
                  }
                : {}),
            },
          }
        : {}),
      ...(llmModel || fallbackSet.size > 0
        ? {
            llm: {
              ...(llmModel ? { model: llmModel } : {}),
              ...(fallbackSet.size > 0
                ? { fallback_models: [...fallbackSet] }
                : {}),
            },
          }
        : {}),
    };
  }

  async function handleSave() {
    if (!configData) return;
    saving = true;
    conflict = false;
    error = null;
    try {
      await saveAgentConfig(agentId, buildConfig(), configData.config_version);
      onClose();
    } catch (e) {
      if (e instanceof Error && e.message === "CONFIG_CONFLICT") {
        conflict = true;
      } else {
        error = e instanceof Error ? e.message : String(e);
      }
    } finally {
      saving = false;
    }
  }

  async function handleReload() {
    conflict = false;
    loading = true;
    await loadAgentConfig(agentId);
    loading = false;
  }
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div class="modal" onclick={(e) => e.stopPropagation()} role="dialog">
    <div class="modal-header">
      <h3>Edit Agent Config</h3>
      <button class="close-btn" onclick={onClose}>&times;</button>
    </div>

    {#if loading}
      <div class="loading">Loading config...</div>
    {:else if configData}
      {#if conflict}
        <div class="conflict-banner">
          Config version conflict — another edit was saved. Reload to get the latest.
          <button onclick={handleReload}>Reload</button>
        </div>
      {/if}
      {#if error}
        <div class="error-banner">{error}</div>
      {/if}

      <div class="form-grid">
        <label>
          <span>Name</span>
          <input type="text" bind:value={name} />
        </label>
        <label>
          <span>Description</span>
          <input type="text" bind:value={description} />
        </label>
        <label>
          <span>Icon</span>
          <input type="text" bind:value={icon} placeholder="emoji or text" />
        </label>
        <label>
          <span>Skills (comma-separated)</span>
          <input type="text" bind:value={skillsText} placeholder="web_search, summarize" />
        </label>
        <label>
          <span>Denied skills (comma-separated)</span>
          <input type="text" bind:value={deniedText} placeholder="shell_execute" />
        </label>
        <label class="full-width">
          <span>Persona</span>
          <textarea bind:value={persona} rows="3"></textarea>
        </label>
        <label>
          <span>Temperature ({temperature.toFixed(2)})</span>
          <input type="range" min="0" max="1" step="0.01" bind:value={temperature} />
        </label>
        <label>
          <span>Verbosity</span>
          <select bind:value={verbosity}>
            <option value="minimal">Minimal</option>
            <option value="normal">Normal</option>
            <option value="detailed">Detailed</option>
          </select>
        </label>

        <div class="section-header">Constraints</div>
        <label>
          <span>Max tool calls</span>
          <input type="number" bind:value={maxToolCalls} placeholder="e.g. 20" />
        </label>
        <label>
          <span>Timeout (seconds)</span>
          <input type="number" bind:value={timeoutSeconds} placeholder="e.g. 300" />
        </label>
        <label>
          <span>Max cost/task ($)</span>
          <input type="number" step="0.01" bind:value={maxCostPerTask} placeholder="e.g. 0.50" />
        </label>
        <label>
          <span>Confirm for (comma-separated)</span>
          <input type="text" bind:value={confirmFor} placeholder="file_delete" />
        </label>

        <div class="section-header">LLM Override</div>
        <label>
          <span>Model</span>
          {#if models.length > 0}
            <select bind:value={llmModel}>
              <option value="">inherit from orchestrator</option>
              {#each models as m (m.id)}
                <option value={m.id}>{m.id} ({m.provider})</option>
              {/each}
              {#if llmModel && !models.find((m) => m.id === llmModel)}
                <option value={llmModel}>{llmModel} (current)</option>
              {/if}
            </select>
          {:else}
            <input type="text" bind:value={llmModel} placeholder="inherit from orchestrator" />
          {/if}
        </label>
        <div class="fallback-section">
          <span>Fallback models</span>
          {#if models.length > 0}
            <div class="checkbox-list">
              {#each models.filter((m) => m.id !== llmModel) as m (m.id)}
                <label class="checkbox-item">
                  <input
                    type="checkbox"
                    checked={fallbackSet.has(m.id)}
                    onchange={() => toggleFallback(m.id)}
                  />
                  <span>{m.id} <span class="provider-tag">{m.provider}</span></span>
                </label>
              {/each}
            </div>
          {:else}
            <input type="text" value={[...fallbackSet].join(", ")} onchange={(e) => {
              fallbackSet = new Set(e.currentTarget.value.split(",").map((s) => s.trim()).filter(Boolean));
            }} />
          {/if}
        </div>
      </div>

      <div class="modal-actions">
        <button onclick={handleSave} disabled={saving}>
          {saving ? "Saving..." : "Save"}
        </button>
        <button class="secondary" onclick={onClose}>Cancel</button>
      </div>
    {:else}
      <div class="loading">Config not found.</div>
    {/if}
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    top: 0; left: 0; width: 100%; height: 100%;
    background: rgba(0, 0, 0, 0.6);
    display: flex; justify-content: center; align-items: center;
    z-index: 1000;
  }
  .modal {
    background: var(--surface);
    padding: 24px; border-radius: 12px;
    width: 90%; max-width: 600px; max-height: 85vh;
    overflow-y: auto;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }
  .modal-header {
    display: flex; justify-content: space-between; align-items: center;
    margin-bottom: 16px;
  }
  .modal-header h3 { margin: 0; font-size: 1.1rem; }
  .close-btn {
    background: none; border: none; color: var(--text-dim);
    font-size: 1.5rem; cursor: pointer; padding: 0; line-height: 1;
  }
  .close-btn:hover { color: var(--text); }

  .conflict-banner {
    background: rgba(245, 158, 11, 0.2);
    border: 1px solid rgba(245, 158, 11, 0.5);
    color: #f59e0b;
    padding: 10px 14px; border-radius: 8px; margin-bottom: 12px;
    font-size: 0.85rem;
    display: flex; justify-content: space-between; align-items: center;
  }
  .conflict-banner button {
    font-size: 0.8rem; padding: 4px 10px;
  }
  .error-banner {
    background: rgba(239, 68, 68, 0.2);
    border: 1px solid var(--error);
    color: var(--error);
    padding: 10px 14px; border-radius: 8px; margin-bottom: 12px;
    font-size: 0.85rem;
  }

  .form-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }
  .form-grid > label {
    display: flex; flex-direction: column; gap: 4px; font-size: 0.85rem;
  }
  .form-grid > label > span {
    color: var(--text-dim); font-weight: 500; font-size: 0.75rem;
    text-transform: uppercase; letter-spacing: 0.3px;
  }
  .full-width { grid-column: 1 / -1; }
  .section-header {
    grid-column: 1 / -1;
    font-size: 0.8rem; font-weight: 600; color: var(--text-dim);
    text-transform: uppercase; letter-spacing: 0.5px;
    margin-top: 8px; padding-bottom: 4px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
  }

  .form-grid input, .form-grid select, .form-grid textarea {
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 6px; padding: 6px 10px;
    color: var(--text); font-size: 0.85rem;
  }
  .form-grid input:focus, .form-grid select:focus, .form-grid textarea:focus {
    outline: none; border-color: var(--accent);
  }
  .form-grid textarea { resize: vertical; font-family: inherit; }
  .form-grid input[type="range"] { padding: 0; }

  .fallback-section {
    grid-column: 1 / -1;
    display: flex; flex-direction: column; gap: 4px; font-size: 0.85rem;
  }
  .fallback-section > span {
    color: var(--text-dim); font-weight: 500; font-size: 0.75rem;
    text-transform: uppercase; letter-spacing: 0.3px;
  }
  .checkbox-list {
    display: flex; flex-direction: column; gap: 4px;
    max-height: 150px; overflow-y: auto;
    padding: 4px 0;
  }
  .checkbox-item {
    display: flex; align-items: center; gap: 8px;
    font-size: 0.82rem; color: var(--text);
    cursor: pointer; padding: 2px 0;
  }
  .checkbox-item input[type="checkbox"] {
    width: 14px; height: 14px; cursor: pointer;
    accent-color: var(--accent);
  }
  .provider-tag {
    font-size: 0.7rem; color: var(--text-dim);
    background: rgba(255, 255, 255, 0.06);
    padding: 1px 6px; border-radius: 3px;
    margin-left: 4px;
  }

  .modal-actions {
    display: flex; justify-content: flex-end; gap: 10px;
    margin-top: 16px; padding-top: 16px;
    border-top: 1px solid rgba(255, 255, 255, 0.05);
  }
  .loading {
    text-align: center; color: var(--text-dim); padding: 40px;
  }
</style>
