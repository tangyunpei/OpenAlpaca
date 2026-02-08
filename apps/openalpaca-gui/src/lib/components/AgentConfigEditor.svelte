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

<div class="fixed inset-0 z-[1000] flex items-center justify-center bg-black/60" onclick={onClose} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div
    class="w-[90%] max-w-[600px] max-h-[85vh] overflow-y-auto rounded-xl border border-border bg-surface p-6 shadow-[0_4px_20px_rgba(0,0,0,0.3)]"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
  >
    <div class="flex items-center justify-between mb-4">
      <h3 class="m-0 text-[1.1rem] font-semibold text-foreground">Edit Agent Config</h3>
      <button
        class="bg-transparent border-none text-muted-foreground text-2xl cursor-pointer p-0 leading-none hover:text-foreground"
        onclick={onClose}
      >&times;</button>
    </div>

    {#if loading}
      <div class="text-center text-muted-foreground py-10">Loading config...</div>
    {:else if configData}
      {#if conflict}
        <div class="flex items-center justify-between bg-amber-500/20 border border-amber-500/50 text-amber-500 px-3.5 py-2.5 rounded-lg mb-3 text-[0.85rem]">
          Config version conflict — another edit was saved. Reload to get the latest.
          <button
            class="text-[0.8rem] px-2.5 py-1 rounded border border-amber-500/50 bg-amber-500/20 text-amber-500 hover:bg-amber-500/30 cursor-pointer"
            onclick={handleReload}
          >Reload</button>
        </div>
      {/if}
      {#if error}
        <div class="bg-danger/20 border border-danger text-danger px-3.5 py-2.5 rounded-lg mb-3 text-[0.85rem]">{error}</div>
      {/if}

      <div class="grid grid-cols-2 gap-3">
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Name</span>
          <input type="text" bind:value={name} class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] focus:outline-none focus:border-accent" />
        </label>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Description</span>
          <input type="text" bind:value={description} class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] focus:outline-none focus:border-accent" />
        </label>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Icon</span>
          <input type="text" bind:value={icon} placeholder="emoji or text" class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] placeholder:text-muted-foreground focus:outline-none focus:border-accent" />
        </label>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Skills (comma-separated)</span>
          <input type="text" bind:value={skillsText} placeholder="web_search, summarize" class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] placeholder:text-muted-foreground focus:outline-none focus:border-accent" />
        </label>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Denied skills (comma-separated)</span>
          <input type="text" bind:value={deniedText} placeholder="shell_execute" class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] placeholder:text-muted-foreground focus:outline-none focus:border-accent" />
        </label>
        <label class="col-span-2 flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Persona</span>
          <textarea bind:value={persona} rows="3" class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] resize-y font-sans focus:outline-none focus:border-accent"></textarea>
        </label>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Temperature ({temperature.toFixed(2)})</span>
          <input type="range" min="0" max="1" step="0.01" bind:value={temperature} class="p-0 accent-accent" />
        </label>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Verbosity</span>
          <select bind:value={verbosity} class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] cursor-pointer focus:outline-none focus:border-accent">
            <option value="minimal">Minimal</option>
            <option value="normal">Normal</option>
            <option value="detailed">Detailed</option>
          </select>
        </label>

        <div class="col-span-2 text-[0.8rem] font-semibold text-muted-foreground uppercase tracking-wider mt-2 pb-1 border-b border-border">Constraints</div>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Max tool calls</span>
          <input type="number" bind:value={maxToolCalls} placeholder="e.g. 20" class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] placeholder:text-muted-foreground focus:outline-none focus:border-accent" />
        </label>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Timeout (seconds)</span>
          <input type="number" bind:value={timeoutSeconds} placeholder="e.g. 300" class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] placeholder:text-muted-foreground focus:outline-none focus:border-accent" />
        </label>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Max cost/task ($)</span>
          <input type="number" step="0.01" bind:value={maxCostPerTask} placeholder="e.g. 0.50" class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] placeholder:text-muted-foreground focus:outline-none focus:border-accent" />
        </label>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Confirm for (comma-separated)</span>
          <input type="text" bind:value={confirmFor} placeholder="file_delete" class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] placeholder:text-muted-foreground focus:outline-none focus:border-accent" />
        </label>

        <div class="col-span-2 text-[0.8rem] font-semibold text-muted-foreground uppercase tracking-wider mt-2 pb-1 border-b border-border">LLM Override</div>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Model</span>
          {#if models.length > 0}
            <select bind:value={llmModel} class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] cursor-pointer focus:outline-none focus:border-accent">
              <option value="">inherit from orchestrator</option>
              {#each models as m (m.id)}
                <option value={m.id}>{m.id} ({m.provider})</option>
              {/each}
              {#if llmModel && !models.find((m) => m.id === llmModel)}
                <option value={llmModel}>{llmModel} (current)</option>
              {/if}
            </select>
          {:else}
            <input type="text" bind:value={llmModel} placeholder="inherit from orchestrator" class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] placeholder:text-muted-foreground focus:outline-none focus:border-accent" />
          {/if}
        </label>
        <div class="col-span-2 flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Fallback models</span>
          {#if models.length > 0}
            <div class="flex flex-col gap-1 max-h-[150px] overflow-y-auto py-1">
              {#each models.filter((m) => m.id !== llmModel) as m (m.id)}
                <label class="flex items-center gap-2 text-[0.82rem] text-foreground cursor-pointer py-0.5">
                  <input
                    type="checkbox"
                    checked={fallbackSet.has(m.id)}
                    onchange={() => toggleFallback(m.id)}
                    class="w-3.5 h-3.5 cursor-pointer accent-accent"
                  />
                  <span>{m.id} <span class="text-[0.7rem] text-muted-foreground bg-white/5 px-1.5 rounded ml-1">{m.provider}</span></span>
                </label>
              {/each}
            </div>
          {:else}
            <input type="text" value={[...fallbackSet].join(", ")} onchange={(e) => {
              fallbackSet = new Set(e.currentTarget.value.split(",").map((s) => s.trim()).filter(Boolean));
            }} class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] focus:outline-none focus:border-accent" />
          {/if}
        </div>
      </div>

      <div class="flex justify-end gap-2.5 mt-4 pt-4 border-t border-border">
        <button
          class="px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/80 disabled:opacity-50 cursor-pointer"
          onclick={handleSave}
          disabled={saving}
        >
          {saving ? "Saving..." : "Save"}
        </button>
        <button
          class="px-4 py-2 rounded-lg bg-white/5 border border-border text-muted-foreground text-sm hover:bg-white/10 cursor-pointer"
          onclick={onClose}
        >Cancel</button>
      </div>
    {:else}
      <div class="text-center text-muted-foreground py-10">Config not found.</div>
    {/if}
  </div>
</div>
