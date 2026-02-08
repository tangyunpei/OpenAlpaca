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

  let agentId = $state("");
  let name = $state("");
  let description = $state("");
  let icon = $state("");
  let skillsText = $state("");
  let persona = $state("You are a helpful assistant.");
  let temperature = $state(0.5);

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

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="fixed inset-0 bg-black/60 flex justify-center items-center z-[1000]" onclick={onClose} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div
    class="bg-surface p-6 rounded-xl w-[90%] max-w-[560px] max-h-[85vh] overflow-y-auto shadow-[0_4px_20px_rgba(0,0,0,0.3)] border border-border"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
  >
    <!-- Header -->
    <div class="flex justify-between items-center mb-3">
      <h3 class="m-0 text-[1.1rem] text-foreground">New Agent</h3>
      <button
        class="bg-transparent border-none text-muted-foreground text-2xl cursor-pointer p-0 leading-none hover:text-foreground"
        onclick={onClose}
      >&times;</button>
    </div>

    <!-- Tabs -->
    <div class="flex mb-4 border-b border-border">
      <button
        class="flex-1 py-2 border-none bg-transparent text-[0.85rem] cursor-pointer border-b-2 transition-colors {activeTab === 'form' ? 'text-accent border-b-accent' : 'text-muted-foreground border-b-transparent'}"
        onclick={() => (activeTab = "form")}
      >
        From Form
      </button>
      <button
        class="flex-1 py-2 border-none bg-transparent text-[0.85rem] cursor-pointer border-b-2 transition-colors {activeTab === 'toml' ? 'text-accent border-b-accent' : 'text-muted-foreground border-b-transparent'}"
        onclick={() => (activeTab = "toml")}
      >
        From TOML
      </button>
    </div>

    <!-- Error -->
    {#if error}
      <div class="bg-danger/20 border border-danger text-danger px-3.5 py-2.5 rounded-lg mb-3 text-[0.85rem]">{error}</div>
    {/if}

    {#if activeTab === "form"}
      <!-- Form grid -->
      <div class="grid grid-cols-2 gap-3">
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Name</span>
          <input
            type="text"
            bind:value={name}
            placeholder="My Agent"
            class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] outline-none focus:border-accent transition-colors"
          />
        </label>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">ID</span>
          <input
            type="text"
            bind:value={agentId}
            placeholder="auto-generated"
            class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] outline-none focus:border-accent transition-colors"
          />
        </label>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Description</span>
          <input
            type="text"
            bind:value={description}
            class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] outline-none focus:border-accent transition-colors"
          />
        </label>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Icon</span>
          <input
            type="text"
            bind:value={icon}
            placeholder="emoji"
            class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] outline-none focus:border-accent transition-colors"
          />
        </label>
        <label class="col-span-2 flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Skills (comma-separated)</span>
          <input
            type="text"
            bind:value={skillsText}
            placeholder="web_search, summarize"
            class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] outline-none focus:border-accent transition-colors"
          />
        </label>
        <label class="col-span-2 flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Persona</span>
          <textarea
            bind:value={persona}
            rows={3}
            class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-[0.85rem] outline-none focus:border-accent resize-y font-[inherit] transition-colors"
          ></textarea>
        </label>
        <label class="flex flex-col gap-1 text-[0.85rem]">
          <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Temperature ({temperature.toFixed(2)})</span>
          <input type="range" min="0" max="1" step="0.01" bind:value={temperature} class="p-0" />
        </label>
      </div>
      <!-- Actions -->
      <div class="flex justify-end gap-2.5 mt-4 pt-4 border-t border-border">
        <button
          class="px-4 py-2 text-sm bg-accent text-accent-foreground border-none rounded-lg cursor-pointer hover:opacity-90 transition-opacity disabled:opacity-50 disabled:cursor-not-allowed"
          onclick={handleCreateFromForm}
          disabled={creating}
        >
          {creating ? "Creating..." : "Create Agent"}
        </button>
        <button
          class="px-4 py-2 text-sm bg-white/5 text-muted-foreground border border-input rounded-lg cursor-pointer hover:bg-white/10 transition-colors"
          onclick={onClose}
        >Cancel</button>
      </div>
    {:else}
      <!-- TOML tab -->
      <label class="flex flex-col gap-1 text-[0.85rem]">
        <span class="text-muted-foreground font-medium text-xs uppercase tracking-wide">Paste TOML config</span>
        <textarea
          bind:value={tomlContent}
          rows={14}
          class="bg-black/30 border border-input rounded-md px-2.5 py-1.5 text-foreground text-xs outline-none focus:border-accent resize-y font-mono transition-colors"
          placeholder={`[agent]\nid = "my_agent"\nname = "My Agent"\ndescription = "..."\n\n[skills]\nassigned = ["web_search"]\n\n[preset]\npersona = "You are a helpful assistant."`}
        ></textarea>
      </label>
      <!-- Actions -->
      <div class="flex justify-end gap-2.5 mt-4 pt-4 border-t border-border">
        <button
          class="px-4 py-2 text-sm bg-accent text-accent-foreground border-none rounded-lg cursor-pointer hover:opacity-90 transition-opacity disabled:opacity-50 disabled:cursor-not-allowed"
          onclick={handleCreateFromToml}
          disabled={creating}
        >
          {creating ? "Creating..." : "Create from TOML"}
        </button>
        <button
          class="px-4 py-2 text-sm bg-white/5 text-muted-foreground border border-input rounded-lg cursor-pointer hover:bg-white/10 transition-colors"
          onclick={onClose}
        >Cancel</button>
      </div>
    {/if}
  </div>
</div>
