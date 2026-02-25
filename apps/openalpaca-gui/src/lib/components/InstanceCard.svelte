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

  function borderAccentStyle(status: string): string {
    const key = getStatusColor(status);
    switch (key) {
      case "status-success":
        return "border-left-color: hsl(160 52% 44%);";
      case "status-warning":
        return "border-left-color: hsl(38 92% 50%);";
      case "status-paused":
        return "border-left-color: hsl(210 70% 55%);";
      case "status-error":
        return "border-left-color: hsl(2 68% 56%);";
      default:
        return "border-left-color: hsl(222 14% 35%);";
    }
  }
</script>

<div
  class="w-[130px] h-[130px] flex flex-col justify-between p-3.5 rounded-2xl border border-white/[0.05] border-l-[3px] transition-all duration-200 hover:scale-[1.03] hover:shadow-lg group cursor-default"
  style="background: linear-gradient(180deg, rgba(255,255,255,0.03) 0%, rgba(255,255,255,0.01) 100%); {borderAccentStyle(instance.status)}"
>
  <!-- Instance ID -->
  <div class="text-[0.68rem] font-mono text-muted-foreground/60 truncate group-hover:text-muted-foreground transition-colors" title={instance.id}>
    ::{shortId}
  </div>

  <!-- Status -->
  <div class="flex items-center gap-2 mt-auto">
    <span class="relative flex items-center justify-center w-2.5 h-2.5">
      <span class="w-2 h-2 rounded-full shrink-0 transition-colors duration-300 {statusDotClass(instance.status)}"></span>
      {#if instance.status === 'busy'}
        <span class="absolute inset-0 rounded-full {statusDotClass(instance.status)} opacity-40 animate-ping" style="animation-duration: 1.5s;"></span>
      {/if}
    </span>
    <span class="text-xs font-medium text-foreground/80 capitalize">{instance.status}</span>
  </div>

  <!-- Task ID (if present) -->
  {#if instance.current_task}
    <div class="text-[0.62rem] text-muted-foreground/50 truncate mt-1.5" title={instance.current_task}>
      <span class="font-mono px-1.5 py-px rounded-md border
                   {instance.status === 'busy' ? 'border-accent/15 text-accent/70' : 'border-white/[0.04] text-muted-foreground/50'}"
            style={instance.status === 'busy' ? 'background: hsl(40 85% 58% / 0.06);' : 'background: rgba(255,255,255,0.025);'}>{instance.current_task.slice(0, 8)}</span>
    </div>
  {:else}
    <div class="h-4"></div>
  {/if}
</div>
