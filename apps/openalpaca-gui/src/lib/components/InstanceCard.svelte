<script lang="ts">
  import type { AgentInstance } from "$lib/types";
  import { getStatusColor } from "$lib/utils";

  interface Props {
    instance: AgentInstance;
  }

  let { instance }: Props = $props();

  /** Extract short ID from instance ID (e.g., "code_agent::a1b2c3d4" → "a1b2c3d4") */
  let shortId = $derived(
    instance.id.includes("::") ? instance.id.split("::").pop() ?? instance.id : instance.id,
  );

  function statusDotClass(status: string): string {
    const key = getStatusColor(status);
    switch (key) {
      case "status-success":
        return "bg-success";
      case "status-warning":
        return "bg-amber-400";
      case "status-paused":
        return "bg-blue-400";
      case "status-error":
        return "bg-danger";
      case "status-dim":
      default:
        return "bg-muted-foreground";
    }
  }

  function borderClass(status: string): string {
    const key = getStatusColor(status);
    switch (key) {
      case "status-success":
        return "border-l-success";
      case "status-warning":
        return "border-l-amber-400";
      case "status-paused":
        return "border-l-blue-400";
      case "status-error":
        return "border-l-danger";
      case "status-dim":
      default:
        return "border-l-muted-foreground";
    }
  }
</script>

<div
  class="w-[120px] h-[120px] flex flex-col justify-between p-3 bg-white/3 rounded-xl border border-white/5 border-l-4 {borderClass(instance.status)} transition-all duration-200 hover:bg-white/6 hover:scale-[1.03] hover:shadow-lg"
>
  <!-- Instance ID -->
  <div class="text-[0.7rem] font-mono text-muted-foreground truncate" title={instance.id}>
    ::{shortId}
  </div>

  <!-- Status -->
  <div class="flex items-center gap-1.5 mt-auto">
    <span class="w-2 h-2 rounded-full shrink-0 transition-colors duration-300 {statusDotClass(instance.status)} {instance.status === 'busy' ? 'animate-pulse' : ''}"></span>
    <span class="text-xs font-medium text-foreground capitalize">{instance.status}</span>
  </div>

  <!-- Task ID (if present) -->
  {#if instance.current_task}
    <div class="text-[0.65rem] text-muted-foreground truncate mt-1" title={instance.current_task}>
      <span class="font-mono bg-white/5 px-1 rounded">{instance.current_task.slice(0, 8)}</span>
    </div>
  {:else}
    <div class="h-4"></div>
  {/if}
</div>
