<script lang="ts">
  import type { Agent, Skill } from "$lib/types";
  import { getStatusColor, safeParseJson, resolveIcon } from "$lib/utils";

  interface Props {
    agent: Agent;
    onclick: () => void;
  }

  let { agent, onclick }: Props = $props();

  let skills = $derived(safeParseJson<Skill[]>(agent.skills_json, []));

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
</script>

<button
  class="block w-full text-left px-5 py-4 bg-white/3 rounded-[10px] mb-2.5 border border-white/5 transition-all duration-200 cursor-pointer text-foreground font-[inherit] text-[inherit] hover:bg-white/6 hover:translate-x-1 hover:border-primary"
  {onclick}
>
  <div class="flex items-center gap-3 mb-2">
    <div class="w-9 h-9 flex items-center justify-center bg-white/5 rounded-lg shrink-0 text-muted-foreground">
      <span class="text-[1.3rem]">{resolveIcon(agent.icon)}</span>
    </div>
    <div class="flex-1 min-w-0">
      <h3 class="m-0 text-base font-semibold overflow-hidden text-ellipsis whitespace-nowrap">{agent.name}</h3>
      {#if agent.description}
        <p class="mt-0.5 mb-0 text-xs text-muted-foreground overflow-hidden text-ellipsis whitespace-nowrap">{agent.description}</p>
      {/if}
    </div>
    <span
      class="w-2.5 h-2.5 rounded-full shrink-0 {statusDotClass(agent.status)}"
      title={agent.status}
    ></span>
  </div>
  {#if agent.current_task_id}
    <div class="text-xs text-muted-foreground mb-2">
      <span class="mr-1">Task:</span>
      <span class="font-mono bg-primary px-1 rounded text-foreground">{agent.current_task_id.slice(0, 8)}</span>
    </div>
  {/if}
  {#if skills.length > 0}
    <div class="flex flex-wrap gap-1.5">
      {#each skills.slice(0, 4) as skill}
        <span class="text-[0.7rem] px-2 py-0.5 rounded-xl bg-accent/12 text-accent font-medium">{skill.name}</span>
      {/each}
      {#if skills.length > 4}
        <span class="text-[0.7rem] px-2 py-0.5 rounded-xl bg-white/5 text-muted-foreground font-medium">+{skills.length - 4}</span>
      {/if}
    </div>
  {/if}
</button>
