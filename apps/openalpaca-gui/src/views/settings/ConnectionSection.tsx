/**
 * Settings → Connection (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * Real: the liveness dot and instance id (`GET /v1/health`), the endpoint (the
 * Tauri `ConnectionInfo`), Reconnect (re-bootstrap + reopen the socket), and
 * today's spend/runs/tokens — all three from `GET /v1/usage/summary`
 * (GAP-08c, closed), which is also where the day itself comes from: the
 * daemon's UTC date rather than the browser's local one, so the runs filter
 * and the spend figure are the same day.
 *
 * Also real: **the project** (plan §4.7 item 2; storage design §6, §9). The
 * daemon reads `x-workspace-path` on `POST /v1/chat`; the Project card is
 * where this window chooses the one it sends (`Use` → `POST /v1/workspaces`),
 * and the Projects list under it (`ProjectsCard.tsx`) is every project the
 * home store records (`GET /v1/workspaces`) with its life-cycle actions. Both
 * live here rather than in a section of their own because the eight sections
 * are the design's (§5.4) and where this daemon files its work is
 * Connection's kind of fact.
 *
 * Also real since Phase 8: **uptime, `Schema vNN` and `Copy log path`** —
 * GAP-14, closed. `GET /v1/status` carries `started_at`/`uptime_secs`, the open
 * database's `schema_version` (not a compile-time count of migration files) and
 * the launcher-managed `log_path`, plus §4.8's two size totals, `retention`
 * (the limits those totals are measured against) and what the boot
 * session-log sweep did. Both launchers — this app's sidecar and
 * `openalpaca daemon start` — write `daemon.log` and claim it (T30); the path
 * is `null` only for a daemon started by hand (a bare `cargo run`), or one
 * that merely found a previous daemon's leftover `daemon.log` at the usual
 * path, and the Copy button is inert there, because a path to a file this
 * daemon did not write is worse than no path.
 *
 * **Stop and Start** (DESIGN_SPEC §5.5, §5.6). `Stop daemon…` opens the
 * confirmation (`StopDaemonDialog`), which re-reads `GET /v1/status` for the
 * `busy` counts before it will stop anything, and the stop itself is
 * `lib/daemon-control.ts`'s: the intent is set and the socket closed *before*
 * the shutdown POST, so no ladder climbs and nothing respawns the daemon.
 * While stopped the card says so in the neutral token, `Reconnect` gives way
 * to `Start daemon` — reconnecting to a daemon that is not there is not a
 * thing to offer — and nothing polls.
 *
 * **Why the daemon would not start** is shown here too, verbatim: the shell's
 * `ensure_daemon_running` rejection ends with what the daemon wrote to its
 * log before it gave up, and `Show daemon log` reads the file directly — both
 * work with no daemon serving anything, which is exactly when they matter.
 *
 * The spend *cap* line is a decision, not a gap: per **N4** there is no daily
 * budget and none is coming, so today's total has no denominator and the
 * design's progress bar stays undrawn. The panel names the two caps the
 * daemon does enforce — per workflow and per agent turn — with the daemon's
 * own numbers.
 */

import { useEffect, useRef, useState } from "react";

import { StopDaemonDialog } from "@/components/overlays/StopDaemonDialog";
import { Button, Eyebrow } from "@/components/ui";
import { useConnectionStatus, useDaemonStatus } from "@/hooks/useConnection";
import { useConnectors } from "@/hooks/useConnectors";
import {
  readDaemonLogTail,
  type StopIntent,
  type StopPhase,
} from "@/lib/connection";
import { stopDaemon, stopToast } from "@/lib/daemon-control";
import { useTasks } from "@/hooks/useTasks";
import { capsNote, formatSpend, useUsageSummary } from "@/hooks/useUsage";
import { useChooseProject } from "@/hooks/useWorkspaces";
import { formatFileSize, type DaemonStatus } from "@/lib/api/types";
import {
  isAbsolutePath,
  RELATIVE_PATH_COPY,
  useProjectStore,
} from "@/stores/project";
import { useUiStore } from "@/stores/ui";

import { relativeTime } from "@/views/library/format";

import { isEnabled as connectorIsOn } from "./ConnectorsSection";
import { Card, GapNote, StatCard, StatusCard } from "./primitives";
import { compactCount, formatUptime } from "./format";
import { ProjectsCard } from "./ProjectsCard";

export function ConnectionSection() {
  const connection = useConnectionStatus();
  const projectPath = useProjectStore((s) => s.path);
  const status = useDaemonStatus(projectPath);
  const summary = useUsageSummary();
  const showToast = useUiStore((s) => s.showToast);
  const [stopOpen, setStopOpen] = useState(false);
  const intent = connection.stopIntent ?? null;
  const stopped = intent !== null;
  // The old process may hold its lock for up to 15 s after the POST; a
  // daemon started in that tail loses the lock race and exits.
  const phase =
    intent === "stopped_here" ? (connection.stopPhase ?? null) : null;
  const stopping = phase === "stopping";

  // Run count for "today" is still a client-side filter — there is no date
  // filter on `GET /v1/tasks` and no rollup that counts runs — but the day it
  // filters on is the daemon's UTC one, off the summary, so all three figures
  // in this card describe the same day. Without a summary there is no day to
  // filter by, and the card says so rather than guessing the local one.
  const tasks = useTasks({ limit: 200 });
  const today = summary.data?.date;
  const runsToday =
    today === undefined
      ? undefined
      : (tasks.data ?? []).filter((task) => task.created_at.startsWith(today))
          .length;

  const logPath = status.data?.log_path ?? null;

  // Both figures are the daemon's own, for the date it named: the total from
  // the `llm_usage_daily` rollup, the tokens from that day's call rows.
  const tokens = (summary.data?.by_provider ?? []).reduce(
    (sum, row) => sum + row.tokens,
    0,
  );

  return (
    <div className="flex flex-col gap-[16px]">
      <StatusCard
        ok={!stopped && connection.connected}
        idle={stopped}
        title={statusTitle(intent, connection.connected, phase)}
        meta={
          stopped
            ? stoppedMeta(connection.stoppedAt ?? null, phase)
            : `uptime ${formatUptime(status.data?.uptime_secs)}`
        }
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
        <div className="mt-[16px] flex flex-wrap gap-[6px]">
          {stopped ? (
            <Button
              variant="secondarySm"
              disabled={stopping}
              title={
                stopping
                  ? "Waiting for the daemon's process to exit and free its lock"
                  : undefined
              }
              onClick={() => {
                if (stopping) return;
                void connection.start();
                showToast("Starting the daemon…");
              }}
            >
              {stopping ? "Stopping…" : "Start daemon"}
            </Button>
          ) : (
            <Button
              variant="secondarySm"
              onClick={() => {
                void connection.reconnect();
                showToast("Reconnecting to the daemon…");
              }}
            >
              Reconnect
            </Button>
          )}
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
          {!stopped && (
            <Button
              variant="ghostSm"
              disabled={!connection.connected}
              onClick={() => setStopOpen(true)}
            >
              Stop daemon…
            </Button>
          )}
        </div>
        {logPath === null && status.data !== undefined && (
          <GapNote>
            This daemon has no daemon.log — the app and `openalpaca daemon
            start` write one for the daemons they launch, and this one was
            started by hand.
          </GapNote>
        )}
        {!stopped && !connection.connected && connection.lastError ? (
          <LogBlock label="Why the daemon is unreachable">
            {connection.lastError}
          </LogBlock>
        ) : null}
        <DaemonLogDisclosure />
      </StatusCard>

      {stopOpen && <StopDaemonFlow onClose={() => setStopOpen(false)} />}

      <StorageCard status={status.data} />

      <StatCard
        title="Today"
        stats={[
          {
            label: "spend",
            value:
              summary.data === undefined
                ? "—"
                : formatSpend(summary.data.total_usd),
          },
          {
            label: "runs",
            value:
              tasks.data === undefined || runsToday === undefined
                ? "—"
                : `${runsToday}`,
          },
          {
            label: "tokens",
            value: summary.data === undefined ? "—" : compactCount(tokens),
          },
        ]}
      >
        {summary.data !== undefined && (
          <GapNote>{capsNote(summary.data.caps)}.</GapNote>
        )}
      </StatCard>

      <ProjectCard
        homeRoot={status.data?.home_root ?? null}
        resolvedRoot={
          status.data === undefined ? undefined : status.data.project_root
        }
      />

      <ProjectsCard currentRoot={status.data?.project_root ?? null} />
    </div>
  );
}

/**
 * The status card's title, stopped or not (§6.3). A stop from this window is
 * titled by what `await_daemon_stopped` concluded, never by the intent alone:
 * the POST's `200` only says the daemon was asked.
 */
export function statusTitle(
  intent: StopIntent,
  connected: boolean,
  phase: StopPhase = null,
): string {
  if (intent === "stopped_here") {
    switch (phase) {
      case "stopping":
        return "Stopping the daemon…";
      case "still_alive":
        return "Daemon did not stop";
      case "lock_still_held":
        return "Daemon exited, lock still held";
      case "unconfirmed":
        return "Daemon stop not confirmed";
      default:
        return "Daemon stopped";
    }
  }
  if (intent === "stopped_elsewhere") return "Daemon stopped from elsewhere";
  return connected ? "Daemon connected" : "Daemon unreachable";
}

function stoppedMeta(stoppedAt: number | null, phase: StopPhase): string {
  if (phase === "stopping") return "waiting for the process to exit";
  // Only a stop the wait confirmed is "stopped"; otherwise it was asked.
  const word = phase === null || phase === "stopped" ? "stopped" : "stop asked";
  if (stoppedAt === null) return word;
  return `${word} ${relativeTime(new Date(stoppedAt).toISOString())}`;
}

/**
 * The confirmation and the stop behind it.
 *
 * Mounted only while open, so its reads run only then: the status is
 * re-asked on open for a current `busy` count, and `Stop daemon` waits for
 * that answer (or its failure) before it can be pressed. The dialog closes on
 * the shutdown POST's response either way; the outcome is a toast, and the
 * window's state is the stop intent's.
 */
function StopDaemonFlow({ onClose }: { onClose: () => void }) {
  const projectPath = useProjectStore((s) => s.path);
  const status = useDaemonStatus(projectPath);
  const connectors = useConnectors();
  const showToast = useUiStore((s) => s.showToast);
  const [reading, setReading] = useState(true);
  const [stopping, setStopping] = useState(false);

  // One fresh read on open. `refetch` settles with the query's own result;
  // whether it answered or failed, the dialog then shows what it has.
  const refetch = useRef(status.refetch);
  useEffect(() => {
    let live = true;
    void Promise.resolve(refetch.current?.()).finally(() => {
      if (live) setReading(false);
    });
    return () => {
      live = false;
    };
  }, []);

  const connectorsRunning = (connectors.data ?? []).some((connector) =>
    connectorIsOn(connector.status),
  );

  return (
    <StopDaemonDialog
      busy={status.data?.busy}
      connectorsRunning={connectorsRunning}
      reading={reading}
      stopping={stopping}
      onCancel={onClose}
      onConfirm={() => {
        setStopping(true);
        void stopDaemon(undefined, onClose).then((result) =>
          showToast(stopToast(result)),
        );
      }}
    />
  );
}

/** How many lines `Show daemon log` reads — the spec's 200. */
export const DAEMON_LOG_LINES = 200;

/**
 * Text from the daemon or its log, shown exactly as written: a `<pre>` with
 * its own scroll, never a toast (a toast is gone in under three seconds, and
 * this is something to read).
 */
function LogBlock({ label, children }: { label: string; children: string }) {
  return (
    <pre
      aria-label={label}
      className="mt-[12px] mb-0 max-h-[320px] overflow-auto rounded-md border border-line-subtle bg-code-chip px-[11px] py-[9px] font-mono text-2xs-plus leading-[1.5] whitespace-pre text-ink"
    >
      {children}
    </pre>
  );
}

/**
 * `Show daemon log` — the end of `daemon.log`, read by the shell straight from
 * the file, for a daemon that started but is misbehaving as much as for one
 * that would not start. Read when opened and again on `Refresh`; never
 * polled.
 */
export function DaemonLogDisclosure() {
  const [open, setOpen] = useState(false);
  const [tail, setTail] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  function load() {
    setError(null);
    readDaemonLogTail(DAEMON_LOG_LINES).then(
      (text) => setTail(text),
      (cause: unknown) => {
        setTail(null);
        setError(cause instanceof Error ? cause.message : String(cause));
      },
    );
  }

  return (
    <div className="mt-[12px]">
      <div className="flex gap-[6px]">
        <Button
          variant="ghostSm"
          aria-expanded={open}
          onClick={() => {
            const next = !open;
            setOpen(next);
            if (next) load();
          }}
        >
          {open ? "Hide daemon log" : "Show daemon log"}
        </Button>
        {open && (
          <Button variant="ghostSm" onClick={load}>
            Refresh
          </Button>
        )}
      </div>
      {open && error !== null && (
        <GapNote>Could not read the daemon log: {error}</GapNote>
      )}
      {open && error === null && tail === "" && (
        <GapNote>The daemon log is empty.</GapNote>
      )}
      {open && error === null && tail !== null && tail !== "" && (
        <LogBlock label="Daemon log">{tail}</LogBlock>
      )}
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
 * naming the cap itself (`retention.log_max_total_bytes`) rather than sending
 * the owner off to raise a number the panel never showed them.
 *
 * Exported for its own tests: it takes a plain `DaemonStatus | undefined`, so
 * it can be rendered without mocking the connection/usage/tasks hooks the
 * rest of the section needs.
 */
export function StorageCard({ status }: { status: DaemonStatus | undefined }) {
  const sweep = status?.sessions.last_sweep ?? null;
  const dropped = status?.sessions.dropped_records ?? 0;
  // The denominator the two `over_cap_after` notes below need — `retention`
  // is what closed Important #1 (T44 fix round 1): without it, "raise the
  // cap" pointed at a number the panel never showed.
  const totalCap = formatFileSize(status?.retention.log_max_total_bytes ?? 0);

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
            ? ` — still over the ${totalCap} cap, with only active sessions left to evict.`
            : "."}
        </GapNote>
      )}
      {sweep !== null && sweep.files_removed === 0 && sweep.over_cap_after && (
        <GapNote>
          Session logs are over the {totalCap} cap and everything left is
          protected — raise log_max_total_bytes or archive some conversations.
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
 * How the home store is named in prose: the daemon's own root when it has
 * said, the generic phrase when it has not (G7).
 */
export function homeStoreLabel(homeRoot: string | null): string {
  return homeRoot === null || homeRoot.trim() === ""
    ? "the home store"
    : `the home store (${homeRoot})`;
}

/**
 * What the Project card names (storage design §9: the resolved root, not the
 * typed text): the root `GET /v1/status` resolved the window's path to; the
 * stored path while the daemon has not answered; and the stored path,
 * flagged, when the daemon answered that it is in no project — a value an
 * earlier build kept as typed, such as a folder an `.openalpaca` directory
 * used to mark.
 */
export function projectCardRoot(
  path: string | null,
  resolved: string | null | undefined,
): { text: string; unresolved: boolean } {
  if (path === null) return { text: "none", unresolved: false };
  if (typeof resolved === "string") {
    return { text: resolved, unresolved: false };
  }
  return { text: path, unresolved: resolved === null };
}

/**
 * The window's project (storage design §3, §9).
 *
 * `Use` asks the daemon what the typed folder is — `POST /v1/workspaces`:
 * the git repository it is in, or, when there is none, the folder itself,
 * which becomes a project (R10) — and keeps the root it answered, never the
 * text typed. A refusal (`WORKSPACE_IS_HOME`, `WORKSPACE_NOT_A_DIRECTORY`,
 * `INVALID_PATH`, `WORKSPACE_REGISTER_FAILED`) is the daemon's sentence,
 * shown as written, and the project the window had stays.
 *
 * Where a run's files go is said plainly: with no project, the home store;
 * with one, its folder under `projects/` in the home store — never the
 * project folder itself (ST-1). The home store is named from
 * `GET /v1/status`'s `home_root`, never from the literal `~/.openalpaca`
 * (G7): `OPENALPACA_HOME_STORE` moves the root, and a path printed from this
 * side would then be a confident lie. A daemon that has not answered yet is
 * "the home store".
 */
function ProjectCard({
  homeRoot,
  resolvedRoot,
}: {
  homeRoot: string | null;
  /** `GET /v1/status.project_root`; `undefined` until the daemon answered. */
  resolvedRoot: string | null | undefined;
}) {
  const path = useProjectStore((s) => s.path);
  const setPath = useProjectStore((s) => s.setPath);
  const showToast = useUiStore((s) => s.showToast);
  const choose = useChooseProject();
  const [draft, setDraft] = useState(path ?? "");
  const [refusal, setRefusal] = useState<string | null>(null);

  const trimmed = draft.trim();
  // Only a genuine typo is an error: an empty box is "no project", not a
  // mistake, and Use is simply inert there.
  const invalid = trimmed !== "" && !isAbsolutePath(trimmed);
  const shown = projectCardRoot(path, resolvedRoot);

  function applyPath() {
    if (trimmed === "" || invalid || choose.isPending) return;
    setRefusal(null);
    choose.mutate(trimmed, {
      onSuccess: (project) => {
        setPath(project.root);
        setDraft(project.root);
        showToast(`Project set to ${project.root} — new runs will use it`);
      },
      onError: (error) => setRefusal(error.message),
    });
  }

  return (
    <Card>
      <div className="flex items-center gap-[10px]">
        <span className="text-md-plus font-semibold text-ink">Project</span>
        <span className="ml-auto font-mono text-xs-plus text-muted-fg">
          {shown.text}
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
            onChange={(event) => {
              setDraft(event.target.value);
              setRefusal(null);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") applyPath();
            }}
            className="min-w-[280px] flex-1 rounded-md border border-line bg-raised px-[8px] py-[4px] font-mono text-2xs-plus text-ink"
          />
          <Button
            variant="primarySm"
            disabled={trimmed === "" || invalid || choose.isPending}
            onClick={applyPath}
          >
            {choose.isPending ? "Using…" : "Use"}
          </Button>
          <Button
            variant="ghostSm"
            disabled={path === null && trimmed === ""}
            onClick={() => {
              setDraft("");
              setRefusal(null);
              setPath(null);
              showToast("Project cleared");
            }}
          >
            Clear
          </Button>
        </div>
      </div>

      {refusal !== null && (
        <p className="mt-[6px] mb-0 text-base leading-[1.6] text-red-ink">
          {refusal}
        </p>
      )}

      {shown.unresolved && (
        <GapNote>
          {`${path} is in no project: no git repository holds it and it was not chosen with Use. Press Use to make it one.`}
        </GapNote>
      )}

      <GapNote>
        {invalid
          ? RELATIVE_PATH_COPY
          : path === null || shown.unresolved
            ? `No project: runs are recorded without one and their files go to ${homeStoreLabel(homeRoot)}.`
            : `Runs started here record this project. What they produce is kept under projects/ in ${homeStoreLabel(homeRoot)}; nothing is written into the project folder.`}
      </GapNote>
    </Card>
  );
}
