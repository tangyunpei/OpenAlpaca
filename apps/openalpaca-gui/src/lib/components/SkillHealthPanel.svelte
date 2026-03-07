<script lang="ts">
  import { skillHealthList, skillsLoading, loadSkillHealth } from "$lib/stores/skills";
  import type { SkillHealthMetrics } from "$lib/types";

  let metrics = $state<SkillHealthMetrics[]>([]);
  let loading = $state(false);

  $effect(() => {
    const unsub = skillHealthList.subscribe((v) => (metrics = v));
    return unsub;
  });
  $effect(() => {
    const unsub = skillsLoading.subscribe((v) => (loading = v));
    return unsub;
  });

  async function refresh() {
    await loadSkillHealth();
  }

  function pct(v: number): string {
    return (v * 100).toFixed(1) + "%";
  }

  function ms(v: number): string {
    if (v < 1000) return Math.round(v) + "ms";
    return (v / 1000).toFixed(1) + "s";
  }

  function cost(v: number): string {
    return "$" + v.toFixed(4);
  }

  function timeAgo(ts: string | null): string {
    if (!ts) return "never";
    const diff = Date.now() - new Date(ts).getTime();
    const mins = Math.floor(diff / 60000);
    if (mins < 1) return "just now";
    if (mins < 60) return mins + "m ago";
    const hours = Math.floor(mins / 60);
    if (hours < 24) return hours + "h ago";
    return Math.floor(hours / 24) + "d ago";
  }

  function rateColor(rate: number): string {
    if (rate >= 0.9) return "text-emerald-400";
    if (rate >= 0.7) return "text-amber-400";
    return "text-orange-400";
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
  <span class="text-[0.65rem] text-muted-foreground/60 font-mono">{metrics.length} skills</span>
</div>

{#if metrics.length === 0 && !loading}
  <div class="text-center py-12 text-muted-foreground/50 text-sm">
    No skill execution data yet. Invoke a skill to see health metrics.
  </div>
{:else}
  <div class="grid gap-3">
    {#each metrics as m (m.skill_id)}
      <div
        class="rounded-xl border border-white/5 p-4"
        style="background: linear-gradient(180deg, rgba(255,255,255,0.03) 0%, rgba(255,255,255,0.01) 100%);"
      >
        <div class="flex items-center justify-between mb-3">
          <span class="text-sm font-semibold text-foreground/90">{m.skill_id}</span>
          <span class="text-[0.6rem] text-muted-foreground/50 font-mono">{timeAgo(m.last_invoked_at)}</span>
        </div>

        <div class="grid grid-cols-3 gap-x-4 gap-y-2 text-xs">
          <div>
            <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">Success</div>
            <div class={rateColor(m.clean_success_rate)}>{pct(m.clean_success_rate)}</div>
          </div>
          <div>
            <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">7d Trend</div>
            <div class={rateColor(m.clean_success_rate_7d)}>
              {pct(m.clean_success_rate_7d)}
              {#if m.clean_success_rate_7d > m.clean_success_rate}
                <span class="text-emerald-400/70">↑</span>
              {:else if m.clean_success_rate_7d < m.clean_success_rate}
                <span class="text-orange-400/70">↓</span>
              {/if}
            </div>
          </div>
          <div>
            <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">Invocations</div>
            <div class="text-foreground/80">{m.total_invocations}</div>
          </div>

          <div>
            <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">Repair</div>
            <div class="text-amber-400/80">{pct(m.repair_rate)}</div>
          </div>
          <div>
            <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">Degraded</div>
            <div class="text-orange-400/80">{pct(m.degraded_rate)}</div>
          </div>
          <div>
            <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">Avg Rounds</div>
            <div class="text-foreground/80">{m.avg_rounds.toFixed(1)}</div>
          </div>

          <div>
            <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">Avg Duration</div>
            <div class="text-foreground/80">{ms(m.avg_duration_ms)}</div>
          </div>
          <div>
            <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">Avg Cost</div>
            <div class="text-foreground/80">{cost(m.avg_cost_usd)}</div>
          </div>
          {#if m.repair_rate > 0}
            <div>
              <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">Repair Eff.</div>
              <div class="text-foreground/80">{pct(m.repair_effectiveness)}</div>
            </div>
          {/if}
          {#if m.feedback_count > 0}
            <div>
              <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">Satisfaction</div>
              <div class={rateColor(m.user_satisfaction_rate ?? 0)}>{pct(m.user_satisfaction_rate ?? 0)}</div>
            </div>
            <div>
              <div class="text-muted-foreground/50 text-[0.6rem] uppercase tracking-wider mb-0.5">Feedback</div>
              <div class="text-foreground/80">{m.feedback_count} <span class="text-muted-foreground/40">({pct(m.feedback_coverage)})</span></div>
            </div>
          {/if}
        </div>
      </div>
    {/each}
  </div>
{/if}
