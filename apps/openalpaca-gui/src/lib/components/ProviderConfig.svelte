<script lang="ts">
  import ApiKeyManager from "./ApiKeyManager.svelte";
  import AddKeyDialog from "./AddKeyDialog.svelte";
  import { cliBackends } from "$lib/stores/settings";
  import type { ProviderInfo, CliBackendStatus } from "$lib/types";

  interface Props {
    providerName: string;
    provider: ProviderInfo;
    onRefresh: () => void;
    onModelsRefresh?: () => void;
  }

  let { providerName, provider, onRefresh, onModelsRefresh }: Props = $props();

  let showAddDialog = $state(false);
  let backends = $state<CliBackendStatus[]>([]);

  const unsubBackends = cliBackends.subscribe((v) => (backends = v));

  function providerDisplayName(name: string): string {
    switch (name) {
      case "anthropic": return "Anthropic (Claude)";
      case "openai": return "OpenAI (GPT)";
      case "ollama": return "Ollama (Local)";
      default: return name;
    }
  }

  function handleKeyAdded() {
    showAddDialog = false;
    onRefresh();
    onModelsRefresh?.();
  }

  function relevantBackend(providerName: string): CliBackendStatus | null {
    if (providerName === "anthropic") return backends.find(b => b.name === "claude_code") ?? null;
    if (providerName === "openai") return backends.find(b => b.name === "codex") ?? null;
    return null;
  }

  $effect(() => {
    return () => unsubBackends();
  });
</script>

<div class="bg-card/70 backdrop-blur-xl rounded-xl p-4 border border-border shadow-[0_4px_16px_rgba(0,0,0,0.2)]">
  <div class="flex items-center justify-between mb-3 flex-wrap gap-2">
    <div class="flex items-center gap-2 flex-wrap">
      <h3 class="m-0 text-base font-semibold text-foreground">{providerDisplayName(providerName)}</h3>
      <span class="text-[0.65rem] px-2 py-0.5 rounded-full uppercase font-bold {provider.enabled ? 'bg-success/20 text-success' : 'bg-gray-500/20 text-gray-400'}">
        {provider.enabled ? "Enabled" : "Disabled"}
      </span>
      <span class="text-[0.65rem] px-2 py-0.5 rounded-full bg-primary/40 text-muted-foreground capitalize">
        {provider.key_selection_strategy.replace(/_/g, ' ')}
      </span>
    </div>
    <button
      class="px-3.5 py-1.5 text-[0.8rem] rounded-md bg-primary text-primary-foreground hover:bg-primary/80 cursor-pointer whitespace-nowrap"
      onclick={() => (showAddDialog = true)}
    >
      + Add Key
    </button>
  </div>

  <ApiKeyManager
    provider={providerName}
    keys={provider.keys}
    {onRefresh}
  />

  {#if relevantBackend(providerName)}
    {@const backend = relevantBackend(providerName)}
    {#if backend}
      <div class="flex items-center gap-1.5 mt-2.5 pt-2.5 border-t border-border text-xs">
        <span class="w-1.5 h-1.5 rounded-full shrink-0 {backend.available ? 'bg-success' : 'bg-muted-foreground'}"></span>
        <span class="text-muted-foreground">CLI Fallback:</span>
        {#if backend.available}
          <span class="text-foreground font-mono text-[0.7rem]">{backend.name} detected{backend.path ? ` at ${backend.path}` : ""}</span>
          {#if !backend.enabled}
            <span class="text-[0.6rem] px-1 rounded-sm bg-amber-500/15 text-amber-500 uppercase">disabled</span>
          {/if}
        {:else}
          <span class="text-muted-foreground opacity-60">{backend.name} not found</span>
        {/if}
      </div>
    {/if}
  {/if}
</div>

{#if showAddDialog}
  <AddKeyDialog
    provider={providerName}
    onclose={() => (showAddDialog = false)}
    onAdded={handleKeyAdded}
  />
{/if}
