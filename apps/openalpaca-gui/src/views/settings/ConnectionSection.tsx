/**
 * Settings → Connection (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * Real: the liveness dot and instance id (`GET /v1/health`), the endpoint (the
 * Tauri `ConnectionInfo`), Reconnect (re-bootstrap + reopen the socket), and
 * today's spend/runs/tokens — spend from `GET /v1/orchestrator/config`'s
 * `daily_cost_usd` (GAP-08a, closed), tokens summed client-side from
 * `GET /v1/llm/usage/daily` (still the only source for those).
 *
 * Unavailable: uptime, `Schema vNN` and `Copy log path` — `/v1/health` is four
 * fields and the migration count is compile-time only (GAP-14) — and the spend
 * *cap*, which nothing serves because there is no daily budget by design (N4:
 * caps are per-workflow/per-turn), so the design's progress bar has no
 * denominator and is omitted rather than drawn against a guess (GAP-08c).
 */

import { Button } from "@/components/ui";
import { useConnectionStatus } from "@/hooks/useConnection";
import { useOrchestratorConfig } from "@/hooks/useOrchestrator";
import { useTasks } from "@/hooks/useTasks";
import { useDaemonStatusDetail } from "@/hooks/useUnbacked";
import { COST_NOTE, useTodaySpend, formatSpend } from "@/hooks/useUsage";
import { todayIsoDate } from "@/lib/api/usage";
import { gapDetail } from "@/lib/unavailable";
import { useUiStore } from "@/stores/ui";

import { GapNote, StatCard, StatusCard } from "./primitives";
import { compactCount } from "./format";

export function ConnectionSection() {
  const connection = useConnectionStatus();
  const detail = useDaemonStatusDetail();
  const orchestrator = useOrchestratorConfig();
  const spend = useTodaySpend();
  const showToast = useUiStore((s) => s.showToast);

  // Run count for "today" is a client-side filter: there is no date filter on
  // `GET /v1/tasks` and no usage rollup that counts runs.
  const tasks = useTasks({ limit: 200 });
  const today = todayIsoDate();
  const runsToday = (tasks.data ?? []).filter((task) =>
    task.created_at.startsWith(today),
  ).length;

  // GAP-14 is permanent until `/v1/status` lands; narrow rather than assume.
  const detailNote = detail.available ? null : gapDetail(detail);

  // Tokens have no source but the daily rollup; cost is the daemon's own
  // authoritative figure (same rows, computed server-side).
  const tokens =
    spend.data === undefined ? 0 : spend.data.tokensIn + spend.data.tokensOut;
  const dailyCostUsd = orchestrator.data?.daily_cost_usd;

  return (
    <div className="flex flex-col gap-[16px]">
      <StatusCard
        ok={connection.connected}
        title={connection.connected ? "Daemon connected" : "Daemon unreachable"}
        meta="uptime —"
        cells={[
          { label: "Instance", value: connection.instanceChip ?? "—" },
          { label: "Endpoint", value: connection.endpoint ?? "—" },
          { label: "Schema", value: "—" },
        ]}
      >
        <div className="mt-[16px] flex gap-[6px]">
          <Button
            variant="secondarySm"
            onClick={() => {
              void connection.reconnect();
              showToast("Reconnecting to the daemon…");
            }}
          >
            Reconnect
          </Button>
          <Button variant="ghostSm" disabled title={detailNote ?? undefined}>
            Copy log path
          </Button>
        </div>
        {detailNote !== null && <GapNote>{detailNote}</GapNote>}
      </StatusCard>

      <StatCard
        title="Today"
        stats={[
          {
            label: "spend",
            value: dailyCostUsd === undefined ? "—" : formatSpend(dailyCostUsd),
          },
          {
            label: "runs",
            value: tasks.data === undefined ? "—" : `${runsToday}`,
          },
          {
            label: "tokens",
            value: spend.data === undefined ? "—" : compactCount(tokens),
          },
        ]}
      >
        <GapNote>{COST_NOTE}.</GapNote>
      </StatCard>
    </div>
  );
}
