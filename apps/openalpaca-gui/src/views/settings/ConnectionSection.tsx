/**
 * Settings → Connection (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * Real: the liveness dot and instance id (`GET /v1/health`), the endpoint (the
 * Tauri `ConnectionInfo`), Reconnect (re-bootstrap + reopen the socket), and
 * today's spend/runs/tokens — spend from `GET /v1/orchestrator/config`'s
 * `daily_cost_usd` (GAP-08a, closed), tokens summed client-side from
 * `GET /v1/llm/usage/daily` (still the only source for those).
 *
 * Also real: **the project** (plan §4.7 item 2). The daemon reads
 * `x-workspace-path` on `POST /v1/chat`; until this field existed no client
 * sent it, so every GUI run was project-less. It lives here rather than in a
 * section of its own because the eight sections are the design's (§5.4) and a
 * single machine-local path is Connection's kind of fact — where this daemon
 * is pointed. There is no project *list* and no browse dialog: the daemon has
 * no project concept to enumerate (plan §10), so the honest control is the one
 * path the owner types.
 *
 * Unavailable: uptime, `Schema vNN` and `Copy log path` — `/v1/health` is four
 * fields and the migration count is compile-time only (GAP-14) — and the spend
 * *cap*, which nothing serves because there is no daily budget by design (N4:
 * caps are per-workflow/per-turn), so the design's progress bar has no
 * denominator and is omitted rather than drawn against a guess (GAP-08c).
 */

import { useState } from "react";

import { Button, Eyebrow } from "@/components/ui";
import { useConnectionStatus } from "@/hooks/useConnection";
import { useOrchestratorConfig } from "@/hooks/useOrchestrator";
import { useTasks } from "@/hooks/useTasks";
import { useDaemonStatusDetail } from "@/hooks/useUnbacked";
import { COST_NOTE, useTodaySpend, formatSpend } from "@/hooks/useUsage";
import { todayIsoDate } from "@/lib/api/usage";
import { gapDetail } from "@/lib/unavailable";
import { isAbsolutePath, useProjectStore } from "@/stores/project";
import { useUiStore } from "@/stores/ui";

import { Card, GapNote, StatCard, StatusCard } from "./primitives";
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

      <ProjectCard />
    </div>
  );
}

/**
 * The chosen project — one absolute path, kept on this machine.
 *
 * What it changes is concrete, so the copy says it: runs started from here
 * record that project and the files they produce land in
 * `<project>/.openalpaca/`. With no project chosen the GUI sends no header at
 * all and the daemon puts everything in the home store — stated plainly rather
 * than left for the owner to discover from a file that appeared in the wrong
 * place.
 */
function ProjectCard() {
  const path = useProjectStore((s) => s.path);
  const setPath = useProjectStore((s) => s.setPath);
  const showToast = useUiStore((s) => s.showToast);
  const [draft, setDraft] = useState(path ?? "");

  const trimmed = draft.trim();
  // Only a genuine typo is an error: an empty box is "no project", not a
  // mistake, and Use is simply inert there.
  const invalid = trimmed !== "" && !isAbsolutePath(trimmed);

  function applyPath() {
    if (trimmed === "" || invalid) return;
    setPath(trimmed);
    showToast("Project set — new runs will use it");
  }

  return (
    <Card>
      <div className="flex items-center gap-[10px]">
        <span className="text-md-plus font-semibold text-ink">Project</span>
        <span className="ml-auto font-mono text-xs-plus text-muted-fg">
          {path ?? "none"}
        </span>
      </div>

      <div className="mt-[16px]">
        <Eyebrow tracking="narrow" tone="faint" className="mb-[4px]">
          workspace path
        </Eyebrow>
        <div className="flex flex-wrap items-center gap-[6px]">
          <input
            aria-label="Project path"
            placeholder="/Users/you/code/your-project"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") applyPath();
            }}
            className="min-w-[280px] flex-1 rounded-md border border-line bg-raised px-[8px] py-[4px] font-mono text-2xs-plus text-ink"
          />
          <Button
            variant="primarySm"
            disabled={trimmed === "" || invalid}
            onClick={applyPath}
          >
            Use
          </Button>
          <Button
            variant="ghostSm"
            disabled={path === null && trimmed === ""}
            onClick={() => {
              setDraft("");
              setPath(null);
              showToast("Project cleared");
            }}
          >
            Clear
          </Button>
        </div>
      </div>

      <GapNote>
        {invalid
          ? "An absolute path, please — a relative one would be read against the daemon's own directory, not this one."
          : path === null
            ? "No project: runs are recorded without one and their files go to the home store (~/.openalpaca)."
            : "Runs started here record this project; files they produce land in <project>/.openalpaca/."}
      </GapNote>
    </Card>
  );
}
