<script lang="ts">
  import { createNewAgent, createAgentFromTomlContent } from "$lib/stores/agents";
  import type { AgentConfigFile } from "$lib/types";

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  let activeTab = $state<"form" | "toml">("form");
  let creating = $state(false);
  let error = $state<string | null>(null);

  // Form tab fields
  let agentId = $state("");
  let name = $state("");
  let description = $state("");
  let icon = $state("");
  let skillsText = $state("");
  let persona = $state("You are a helpful assistant.");
  let temperature = $state(0.5);

  // TOML tab
  let tomlContent = $state("");

  function generateId(name: string): string {
    return name.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_|_$/g, "") || "new_agent";
  }

  $effect(() => {
    if (!agentId && name) {
      agentId = generateId(name);
    }
  });

  async function handleCreateFromForm() {
    if (!agentId || !name || !persona) {
      error = "ID, Name, and Persona are required.";
      return;
    }
    creating = true;
    error = null;
    try {
      const config: AgentConfigFile = {
        agent: { id: agentId, name, description, ...(icon ? { icon } : {}) },
        skills: {
          assigned: skillsText.split(",").map((s) => s.trim()).filter(Boolean),
        },
        preset: { persona, temperature },
      };
      await createNewAgent(config);
      onClose();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      creating = false;
    }
  }

  async function handleCreateFromToml() {
    if (!tomlContent.trim()) {
      error = "TOML content is required.";
      return;
    }
    creating = true;
    error = null;
    try {
      await createAgentFromTomlContent(tomlContent);
      onClose();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      creating = false;
    }
  }
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div class="modal" onclick={(e) => e.stopPropagation()} role="dialog">
    <div class="modal-header">
      <h3>New Agent</h3>
      <button class="close-btn" onclick={onClose}>&times;</button>
    </div>

    <div class="tabs">
      <button class:active={activeTab === "form"} onclick={() => (activeTab = "form")}>
        From Form
      </button>
      <button class:active={activeTab === "toml"} onclick={() => (activeTab = "toml")}>
        From TOML
      </button>
    </div>

    {#if error}
      <div class="error-banner">{error}</div>
    {/if}

    {#if activeTab === "form"}
      <div class="form-grid">
        <label>
          <span>Name</span>
          <input type="text" bind:value={name} placeholder="My Agent" />
        </label>
        <label>
          <span>ID</span>
          <input type="text" bind:value={agentId} placeholder="auto-generated" />
        </label>
        <label>
          <span>Description</span>
          <input type="text" bind:value={description} />
        </label>
        <label>
          <span>Icon</span>
          <input type="text" bind:value={icon} placeholder="emoji" />
        </label>
        <label class="full-width">
          <span>Skills (comma-separated)</span>
          <input type="text" bind:value={skillsText} placeholder="web_search, summarize" />
        </label>
        <label class="full-width">
          <span>Persona</span>
          <textarea bind:value={persona} rows="3"></textarea>
        </label>
        <label>
          <span>Temperature ({temperature.toFixed(2)})</span>
          <input type="range" min="0" max="1" step="0.01" bind:value={temperature} />
        </label>
      </div>
      <div class="modal-actions">
        <button onclick={handleCreateFromForm} disabled={creating}>
          {creating ? "Creating..." : "Create Agent"}
        </button>
        <button class="secondary" onclick={onClose}>Cancel</button>
      </div>
    {:else}
      <label class="full-width toml-label">
        <span>Paste TOML config</span>
        <textarea bind:value={tomlContent} rows="14" class="toml-editor" placeholder={`[agent]\nid = "my_agent"\nname = "My Agent"\ndescription = "..."\n\n[skills]\nassigned = ["web_search"]\n\n[preset]\npersona = "You are a helpful assistant."`}></textarea>
      </label>
      <div class="modal-actions">
        <button onclick={handleCreateFromToml} disabled={creating}>
          {creating ? "Creating..." : "Create from TOML"}
        </button>
        <button class="secondary" onclick={onClose}>Cancel</button>
      </div>
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
    width: 90%; max-width: 560px; max-height: 85vh;
    overflow-y: auto;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }
  .modal-header {
    display: flex; justify-content: space-between; align-items: center;
    margin-bottom: 12px;
  }
  .modal-header h3 { margin: 0; font-size: 1.1rem; }
  .close-btn {
    background: none; border: none; color: var(--text-dim);
    font-size: 1.5rem; cursor: pointer; padding: 0; line-height: 1;
  }
  .close-btn:hover { color: var(--text); }

  .tabs {
    display: flex; gap: 0; margin-bottom: 16px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  }
  .tabs button {
    flex: 1; padding: 8px; border: none; background: none;
    color: var(--text-dim); font-size: 0.85rem; cursor: pointer;
    border-bottom: 2px solid transparent;
  }
  .tabs button.active {
    color: var(--accent); border-bottom-color: var(--accent);
  }

  .error-banner {
    background: rgba(239, 68, 68, 0.2);
    border: 1px solid var(--error);
    color: var(--error);
    padding: 10px 14px; border-radius: 8px; margin-bottom: 12px;
    font-size: 0.85rem;
  }

  .form-grid {
    display: grid; grid-template-columns: 1fr 1fr; gap: 12px;
  }
  .form-grid label, .toml-label {
    display: flex; flex-direction: column; gap: 4px; font-size: 0.85rem;
  }
  .form-grid label span, .toml-label span {
    color: var(--text-dim); font-weight: 500; font-size: 0.75rem;
    text-transform: uppercase; letter-spacing: 0.3px;
  }
  .full-width { grid-column: 1 / -1; }

  .form-grid input, .form-grid select, .form-grid textarea,
  .toml-editor {
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 6px; padding: 6px 10px;
    color: var(--text); font-size: 0.85rem;
  }
  .form-grid input:focus, .form-grid textarea:focus,
  .toml-editor:focus {
    outline: none; border-color: var(--accent);
  }
  .form-grid textarea, .toml-editor { resize: vertical; font-family: inherit; }
  .toml-editor { font-family: "Fira Code", monospace; font-size: 0.8rem; }
  .form-grid input[type="range"] { padding: 0; }
  .toml-label { margin-bottom: 0; }

  .modal-actions {
    display: flex; justify-content: flex-end; gap: 10px;
    margin-top: 16px; padding-top: 16px;
    border-top: 1px solid rgba(255, 255, 255, 0.05);
  }
</style>
