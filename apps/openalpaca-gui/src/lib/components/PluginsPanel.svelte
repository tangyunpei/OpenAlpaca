<script lang="ts">
  import { pluginList, pluginsLoading, loadPlugins } from "$lib/stores/plugins";
  import { approvePlugin, denyPlugin, enablePlugin, disablePlugin } from "$lib/api/plugins";
  import type { PluginInfo } from "$lib/api/plugins";

  let plugins = $state<PluginInfo[]>([]);
  let loading = $state(false);
  let actionInFlight = $state<string | null>(null);

  $effect(() => {
    const unsub = pluginList.subscribe((v) => (plugins = v));
    return unsub;
  });
  $effect(() => {
    const unsub = pluginsLoading.subscribe((v) => (loading = v));
    return unsub;
  });

  async function refresh() {
    await loadPlugins();
  }

  async function handleApprove(name: string) {
    actionInFlight = name;
    try {
      await approvePlugin(name);
      await loadPlugins();
    } catch (e) {
      console.error(`Failed to approve plugin ${name}:`, e);
    } finally {
      actionInFlight = null;
    }
  }

  async function handleDeny(name: string) {
    actionInFlight = name;
    try {
      await denyPlugin(name);
      await loadPlugins();
    } catch (e) {
      console.error(`Failed to deny plugin ${name}:`, e);
    } finally {
      actionInFlight = null;
    }
  }

  async function handleEnable(name: string) {
    actionInFlight = name;
    try {
      await enablePlugin(name);
      await loadPlugins();
    } catch (e) {
      console.error(`Failed to enable plugin ${name}:`, e);
    } finally {
      actionInFlight = null;
    }
  }

  async function handleDisable(name: string) {
    actionInFlight = name;
    try {
      await disablePlugin(name);
      await loadPlugins();
    } catch (e) {
      console.error(`Failed to disable plugin ${name}:`, e);
    } finally {
      actionInFlight = null;
    }
  }

  function statusColor(status: string): string {
    if (status === "running") return "text-emerald-400";
    if (status === "waiting_approval" || status === "needs_config") return "text-amber-400";
    if (status === "crashed" || status === "disabled") return "text-red-400";
    return "text-muted-foreground/60";
  }

  function statusBg(status: string): string {
    if (status === "running") return "bg-emerald-400/15 border-emerald-400/30";
    if (status === "waiting_approval" || status === "needs_config") return "bg-amber-400/15 border-amber-400/30";
    if (status === "crashed" || status === "disabled") return "bg-red-400/15 border-red-400/30";
    return "bg-white/5 border-white/10";
  }
</script>

<div class="flex flex-wrap gap-3 mb-5 items-center">
  <button
    class="px-4 py-2 rounded-xl border border-white/5 text-sm font-medium text-foreground/80 cursor-pointer transition-all duration-200 hover:border-white/10 disabled:opacity-40 disabled:cursor-not-allowed"
    style="background: linear-gradient(180deg, rgba(255,255,255,0.04) 0%, rgba(255,255,255,0.02) 100%);"
    onclick={refresh}
    disabled={loading}
  >
    {loading ? "Loading..." : "Refresh"}
  </button>
  <span class="text-[0.65rem] text-muted-foreground/60 font-mono">{plugins.length} plugins</span>
</div>

{#if plugins.length === 0 && !loading}
  <div class="flex flex-col items-center justify-center py-16 px-10 text-center animate-fadeIn">
    <div class="w-14 h-14 mb-4 rounded-2xl flex items-center justify-center border border-white/5"
         style="background: linear-gradient(135deg, rgba(255,255,255,0.03) 0%, rgba(255,255,255,0.01) 100%);">
      <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class="text-muted-foreground/40">
        <path d="M12 2L2 7l10 5 10-5-10-5z"/><path d="M2 17l10 5 10-5"/><path d="M2 12l10 5 10-5"/>
      </svg>
    </div>
    <p class="text-foreground/60 text-sm font-medium m-0">No plugins installed</p>
    <p class="text-muted-foreground/40 text-xs m-0 mt-1.5 leading-relaxed">Plugins will appear here when loaded by the daemon</p>
  </div>
{:else}
  <div class="grid gap-3">
    {#each plugins as plugin, idx (plugin.name)}
      <div
        class="rounded-xl border border-white/5 p-4"
        style="background: linear-gradient(180deg, rgba(255,255,255,0.03) 0%, rgba(255,255,255,0.01) 100%); animation: slideUp 0.35s cubic-bezier(0.16, 1, 0.3, 1) both; animation-delay: {idx * 50}ms;"
      >
        <!-- Header: name + version + status -->
        <div class="flex items-center justify-between mb-3">
          <div class="flex items-center gap-2">
            <span class="text-sm font-semibold text-foreground/90">{plugin.name}</span>
            <span class="text-[0.6rem] text-muted-foreground/50 font-mono">v{plugin.version}</span>
          </div>
          <span class="text-[0.6rem] font-medium px-2 py-0.5 rounded-full border {statusBg(plugin.status)} {statusColor(plugin.status)}">
            {plugin.status}
          </span>
        </div>

        <!-- Details grid -->
        <div class="grid grid-cols-1 gap-2 text-xs mb-3">
          {#if plugin.tools.length > 0}
            <div>
              <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">Tools</div>
              <div class="text-foreground/80">{plugin.tools.join(", ")}</div>
            </div>
          {/if}
          {#if plugin.connector}
            <div>
              <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">Connector</div>
              <div class="text-foreground/80">{plugin.connector}</div>
            </div>
          {/if}
          {#if plugin.provider}
            <div>
              <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">Provider</div>
              <div class="text-foreground/80">
                {plugin.provider}
                {#if plugin.models.length > 0}
                  <span class="text-muted-foreground/40 ml-1">({plugin.models.join(", ")})</span>
                {/if}
              </div>
            </div>
          {/if}
        </div>

        <!-- Action buttons -->
        <div class="flex gap-2 flex-wrap">
          {#if plugin.status.includes("waiting_approval")}
            <button
              class="action-btn px-3 py-1.5 rounded-lg text-xs font-medium border border-emerald-400/30 text-emerald-400 cursor-pointer transition-all duration-200 hover:bg-emerald-400/15 disabled:opacity-40 disabled:cursor-not-allowed"
              style="background: rgba(52, 211, 153, 0.08);"
              onclick={() => handleApprove(plugin.name)}
              disabled={actionInFlight === plugin.name}
            >Approve</button>
            <button
              class="action-btn px-3 py-1.5 rounded-lg text-xs font-medium border border-red-400/30 text-red-400 cursor-pointer transition-all duration-200 hover:bg-red-400/15 disabled:opacity-40 disabled:cursor-not-allowed"
              style="background: rgba(248, 113, 113, 0.08);"
              onclick={() => handleDeny(plugin.name)}
              disabled={actionInFlight === plugin.name}
            >Deny</button>
          {/if}
          {#if plugin.status === "disabled" || plugin.status.includes("crashed")}
            <button
              class="action-btn px-3 py-1.5 rounded-lg text-xs font-medium border border-emerald-400/30 text-emerald-400 cursor-pointer transition-all duration-200 hover:bg-emerald-400/15 disabled:opacity-40 disabled:cursor-not-allowed"
              style="background: rgba(52, 211, 153, 0.08);"
              onclick={() => handleEnable(plugin.name)}
              disabled={actionInFlight === plugin.name}
            >Enable</button>
          {/if}
          {#if plugin.status === "running"}
            <button
              class="action-btn px-3 py-1.5 rounded-lg text-xs font-medium border border-amber-400/30 text-amber-400 cursor-pointer transition-all duration-200 hover:bg-amber-400/15 disabled:opacity-40 disabled:cursor-not-allowed"
              style="background: rgba(251, 191, 36, 0.08);"
              onclick={() => handleDisable(plugin.name)}
              disabled={actionInFlight === plugin.name}
            >Disable</button>
          {/if}
        </div>
      </div>
    {/each}
  </div>
{/if}
