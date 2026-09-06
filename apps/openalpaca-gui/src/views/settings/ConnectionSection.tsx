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
 * Also real since Phase 8: **uptime, `Schema vNN` and `Copy log path`** —
 * GAP-14, closed. `GET /v1/status` carries `started_at`/`uptime_secs`, the open
 * database's `schema_version` (not a compile-time count of migration files) and
 * the CLI-managed `log_path`, plus §4.8's two size totals and what the boot
 * session-log sweep did. The log path is `null` for a daemon the CLI did not
 * start — the sidecar, a `cargo run` — and the Copy button is inert there,
 * because a path to a file that was never written is worse than no path.
 *
 * Unavailable: the spend *cap*, which nothing serves because there is no daily
 * budget by design (N4: caps are per-workflow/per-turn), so the design's
 * progress bar has no denominator and is omitted rather than drawn against a
 * guess (GAP-08c).
 */

import { useState } from "react";

import { Button, Eyebrow } from "@/components/ui";
import { useConnectionStatus, useDaemonStatus } from "@/hooks/useConnection";
import { useOrchestratorConfig } from "@/hooks/useOrchestrator";
import { useTasks } from "@/hooks/useTasks";
import { COST_NOTE, useTodaySpend, formatSpend } from "@/hooks/useUsage";
import { formatFileSize, type DaemonStatus } from "@/lib/api/types";
import { todayIsoDate } from "@/lib/api/usage";
import { isAbsolutePath, useProjectStore } from "@/stores/project";
import { useUiStore } from "@/stores/ui";

import { Card, GapNote, StatCard, StatusCard } from "./primitives";
import { compactCount, formatUptime } from "./format";

export function ConnectionSection() {
  const connection = useConnectionStatus();
  const projectPath = useProjectStore((s) => s.path);
  const status = useDaemonStatus(projectPath);
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

  const logPath = status.data?.log_path ?? null;

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
        meta={`uptime ${formatUptime(status.data?.uptime_secs)}`}
        cells={[
          { label: "Instance", value: connection.instanceChip ?? "—" },
          { label: "Endpoint", value: connection.endpoint ?? "—" },
          {
            label: "Schema",
            value:
              status.data === undefined
                ? "—"
                : `v${status.data.schema_version}`,
          },
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
          <Button
            variant="ghostSm"
            disabled={logPath === null}
            title={logPath ?? undefined}
            onClick={() => {
              if (logPath === null) return;
              void navigator.clipboard.writeText(logPath);
              showToast("Log path copied to the clipboard");
            }}
          >
            Copy log path
          </Button>
        </div>
        {logPath === null && status.data !== undefined && (
          <GapNote>
            This daemon has no daemon.log — the log file is written by
            `openalpaca daemon start`, not by a daemon the app launched itself.
          </GapNote>
        )}
      </StatusCard>

      <StorageCard status={status.data} />

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
 * What the daemon's storage costs — §4.8's two numbers, never one.
 *
 * `uploads` is the quota-bearing total the daemon checks an upload against;
 * `produced` is agent output, informational and never charged. They are kept
 * apart here for the same reason the daemon keeps them apart: a run that wrote
 * a large artifact must not look like it consumed the upload allowance.
 *
 * The session line under them is the boot sweep's own account. It is silent on
 * a daemon whose sweep found nothing to do — an eviction that never happened
 * is not news — and says so plainly when the log is *still* over its cap,
 * which is the one state the owner can act on.
 */
function StorageCard({ status }: { status: DaemonStatus | undefined }) {
  const sweep = status?.sessions.last_sweep ?? null;
  const dropped = status?.sessions.dropped_records ?? 0;

  return (
    <StatCard
      title="Storage"
      stats={[
        {
          label: "uploads",
          value:
            status === undefined ? "—" : formatFileSize(status.upload_bytes),
        },
        {
          label: "produced",
          value:
            status === undefined ? "—" : formatFileSize(status.produced_bytes),
        },
      ]}
    >
      {sweep !== null && sweep.files_removed > 0 && (
        <GapNote>
          Session logs: this boot's sweep freed{" "}
          {formatFileSize(sweep.bytes_freed)} from {sweep.sessions_evicted} of{" "}
          {sweep.sessions_visited} sessions
          {sweep.over_cap_after
            ? " — still over the total cap, with only active sessions left to evict."
            : "."}
        </GapNote>
      )}
      {sweep !== null && sweep.files_removed === 0 && sweep.over_cap_after && (
        <GapNote>
          Session logs are over the total cap and everything left is protected —
          raise `log_max_total_bytes` or archive some conversations.
        </GapNote>
      )}
      {dropped > 0 && (
        <GapNote>
          {dropped} session-log records were dropped this boot: those
          transcripts have gaps.
        </GapNote>
      )}
    </StatCard>
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
