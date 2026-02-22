<script lang="ts">
  import { onMount } from "svelte";
  import {
    daemonProviders,
    loadDaemonProviders,
    saveDaemonWebSearch,
    subscribeToDaemonConfigEvents,
  } from "$lib/stores/settings";
  import type { DaemonProvidersResponse } from "$lib/api/daemon";

  let config = $state<DaemonProvidersResponse | null>(null);
  let apiKeyInput = $state("");
  let timeoutInput = $state(10);
  let showKey = $state(false);
  let saving = $state(false);
  let error = $state<string | null>(null);
  let success = $state(false);

  const unsubConfig = daemonProviders.subscribe((v) => {
    config = v;
    if (v) {
      timeoutInput = v.web_search.timeout_secs;
    }
  });

  let unsubEvents: (() => void) | null = null;

  onMount(() => {
    loadDaemonProviders();
    unsubEvents = subscribeToDaemonConfigEvents();

    return () => {
      unsubConfig();
      unsubEvents?.();
    };
  });

  async function handleSave() {
    saving = true;
    error = null;
    success = false;
    try {
      await saveDaemonWebSearch(
        apiKeyInput.trim() || undefined,
        timeoutInput,
      );
      apiKeyInput = "";
      success = true;
      setTimeout(() => (success = false), 3000);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  async function handleClearKey() {
    saving = true;
    error = null;
    try {
      await saveDaemonWebSearch("", undefined);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }
</script>

<div
  class="bg-card/70 backdrop-blur-xl rounded-xl p-4 border border-border shadow-[0_4px_16px_rgba(0,0,0,0.2)]"
>
  <div class="flex items-center justify-between mb-3 flex-wrap gap-2">
    <div class="flex items-center gap-2 flex-wrap">
      <h3 class="m-0 text-base font-semibold text-foreground">
        Web Search (Brave)
      </h3>
      {#if config}
        <span
          class="text-[0.65rem] px-2 py-0.5 rounded-full uppercase font-bold {config
            .web_search.api_key_configured
            ? 'bg-success/20 text-success'
            : 'bg-gray-500/20 text-gray-400'}"
        >
          {config.web_search.api_key_configured
            ? "Configured"
            : "Not Configured"}
        </span>
      {/if}
    </div>
  </div>

  {#if config}
    <!-- API Key section -->
    <div class="space-y-3">
      {#if config.web_search.api_key_configured}
        <div class="flex items-center gap-2 text-sm">
          <span class="text-muted-foreground">API Key:</span>
          <code
            class="bg-white/10 px-2 py-0.5 rounded text-xs font-mono text-foreground"
            >{config.web_search.api_key_hint}</code
          >
          <button
            class="text-[0.7rem] px-2 py-0.5 rounded bg-danger/20 text-danger hover:bg-danger/30 cursor-pointer border-none"
            onclick={handleClearKey}
            disabled={saving}
          >
            Remove
          </button>
        </div>
      {/if}

      <div class="flex gap-2 items-end">
        <div class="flex-1">
          <label
            class="block text-xs text-muted-foreground mb-1"
            for="ws-api-key"
          >
            {config.web_search.api_key_configured
              ? "Replace API Key"
              : "API Key"}
          </label>
          <div class="relative">
            <input
              id="ws-api-key"
              type={showKey ? "text" : "password"}
              bind:value={apiKeyInput}
              placeholder="BSA..."
              class="w-full px-3 py-1.5 text-sm rounded-md bg-white/5 border border-border text-foreground placeholder:text-muted-foreground/50 focus:outline-none focus:border-accent"
            />
            <button
              class="absolute right-2 top-1/2 -translate-y-1/2 text-[0.65rem] text-muted-foreground hover:text-foreground cursor-pointer bg-transparent border-none"
              onclick={() => (showKey = !showKey)}
              type="button"
            >
              {showKey ? "Hide" : "Show"}
            </button>
          </div>
        </div>
      </div>

      <!-- Timeout -->
      <div>
        <label
          class="block text-xs text-muted-foreground mb-1"
          for="ws-timeout"
        >
          Timeout (seconds)
        </label>
        <input
          id="ws-timeout"
          type="number"
          min="1"
          max="60"
          bind:value={timeoutInput}
          class="w-20 px-3 py-1.5 text-sm rounded-md bg-white/5 border border-border text-foreground focus:outline-none focus:border-accent"
        />
        <span class="text-xs text-muted-foreground ml-2">1–60</span>
      </div>

      <!-- Save button + feedback -->
      <div class="flex items-center gap-2 pt-1">
        <button
          class="px-4 py-1.5 text-[0.8rem] rounded-md bg-primary text-primary-foreground hover:bg-primary/80 cursor-pointer disabled:opacity-50"
          onclick={handleSave}
          disabled={saving}
        >
          {saving ? "Saving..." : "Save"}
        </button>
        {#if success}
          <span class="text-xs text-success">Saved!</span>
        {/if}
        {#if error}
          <span class="text-xs text-danger">{error}</span>
        {/if}
      </div>

      <p class="text-[0.7rem] text-muted-foreground/60 mt-1">
        Get an API key at
        <a
          href="https://brave.com/search/api/"
          target="_blank"
          rel="noopener"
          class="text-accent hover:underline">brave.com/search/api</a
        >
      </p>
    </div>
  {:else}
    <div class="text-muted-foreground text-sm py-2">Loading...</div>
  {/if}
</div>
