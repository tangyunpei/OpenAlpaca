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
import { readDaemonLogTail } from "@/lib/connection";
import { stopDaemon, stopToast } from "@/lib/daemon-control";
import { useTasks } from "@/hooks/useTasks";
import { capsNote, formatSpend, useUsageSummary } from "@/hooks/useUsage";
import { useMovedProject, useRebaseWorkspace } from "@/hooks/useWorkspaces";
import { formatFileSize, type DaemonStatus } from "@/lib/api/types";
import { isAbsolutePath, useProjectStore } from "@/stores/project";
import { useUiStore } from "@/stores/ui";

import { relativeTime } from "@/views/library/format";

import { isEnabled as connectorIsOn } from "./ConnectorsSection";
import { Card, GapNote, StatCard, StatusCard } from "./primitives";
import { compactCount, formatUptime } from "./format";

export function ConnectionSection() {
  const connection = useConnectionStatus();
  const projectPath = useProjectStore((s) => s.path);
  const status = useDaemonStatus(projectPath);
  const summary = useUsageSummary();
  const showToast = useUiStore((s) => s.showToast);
  const [stopOpen, setStopOpen] = useState(false);
  const intent = connection.stopIntent ?? null;
  const stopped = intent !== null;

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
        title={statusTitle(intent, connection.connected)}
        meta={
          stopped
            ? stoppedMeta(connection.stoppedAt ?? null)
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
              onClick={() => {
                void connection.start();
                showToast("Starting the daemon…");
              }}
            >
              Start daemon
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

      <ProjectCard homeRoot={status.data?.home_root ?? null} />
    </div>
  );
}

/** The status card's title, stopped or not (§6.3). */
export function statusTitle(
  intent: "stopped_here" | "stopped_elsewhere" | null,
  connected: boolean,
): string {
  if (intent === "stopped_here") return "Daemon stopped";
  if (intent === "stopped_elsewhere") return "Daemon stopped from elsewhere";
  return connected ? "Daemon connected" : "Daemon unreachable";
}

function stoppedMeta(stoppedAt: number | null): string {
  if (stoppedAt === null) return "stopped";
  return `stopped ${relativeTime(new Date(stoppedAt).toISOString())}`;
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
 * The chosen project — one absolute path, kept on this machine.
 *
 * What it changes is concrete, so the copy says it: runs started from here
 * record that project and the files they produce land in
 * `<project>/.openalpaca/`. With no project chosen the GUI sends no header at
 * all and the daemon puts everything in the home store — stated plainly rather
 * than left for the owner to discover from a file that appeared in the wrong
 * place.
 *
 * The home store is named from `GET /v1/status`'s `home_root`, never from the
 * literal `~/.openalpaca` (G7): `OPENALPACA_HOME_STORE` moves the root, and a
 * path printed from this side would then be a confident lie about where the
 * owner's files went. A daemon that has not answered yet is "the home store".
 */
function ProjectCard({ homeRoot }: { homeRoot: string | null }) {
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

      <MovedProjectOffer path={path} />

      <GapNote>
        {invalid
          ? "An absolute path, please — a relative one would be read against the daemon's own directory, not this one."
          : path === null
            ? `No project: runs are recorded without one and their files go to ${homeStoreLabel(homeRoot)}.`
            : "Runs started here record this project; files they produce land in <project>/.openalpaca/."}
      </GapNote>
    </Card>
  );
}

/**
 * The moved-project offer (plan §4.8, P-12).
 *
 * A project's path is its identity in four places — its artifacts, its
 * conversations, its runs and its workspace memories — so moving the directory
 * strands all four at once. The daemon can re-attach them in one transaction,
 * but it cannot know where the project went; the owner does, and has just said
 * so by choosing this path.
 *
 * So this offers, and never acts. The detection is exact (the store records the
 * root it was seeded at, and one standing at a path it does not name has
 * moved), the counts are the daemon's, and the re-base is one deliberate
 * confirmation away. Auto-re-basing on a path the owner merely typed would
 * re-address someone's whole history on a guess.
 *
 * Every state is stated rather than hidden: checking, a lookup that failed, a
 * project that has not moved (nothing at all), the offer, and a refusal with
 * the daemon's own reason — `WORKSPACE_BUSY` while a run is in flight,
 * `WORKSPACE_EXISTS` when the new root already has a history of its own,
 * `WORKSPACE_NOT_A_ROOT` when the path chosen here sits inside another
 * project's root (the daemon names that root, and this shows what it said),
 * `WORKSPACE_IS_HOME` when it resolves to the home store, and
 * `WORKSPACE_NOT_FOUND` when the rows there belong to another owner. The
 * daemon's sentence is rendered verbatim: it is the only thing that knows
 * which root it resolved.
 *
 * `WORKSPACE_BUSY` is the one refusal this card can see coming, because
 * `GET /v1/workspaces` already counts the runs in flight under the old root.
 * The copy used to say a re-base "waits until they finish"; it does not — the
 * daemon answers `409` and changes nothing — so the sentence says that, and
 * the button is disabled while the count is above zero rather than offering a
 * press that is already known to be refused.
 */
function MovedProjectOffer({ path }: { path: string | null }) {
  const { moved, pending, error } = useMovedProject(path);
  const rebase = useRebaseWorkspace();
  const showToast = useUiStore((s) => s.showToast);
  const [confirming, setConfirming] = useState(false);

  // A new project (or a completed re-base) puts the confirmation away: the
  // question it was asking is no longer the one on screen.
  const offerKey = moved === null ? null : `${moved.from}→${moved.to}`;
  const [lastOffer, setLastOffer] = useState(offerKey);
  if (lastOffer !== offerKey) {
    setLastOffer(offerKey);
    setConfirming(false);
  }

  if (path === null) return null;
  if (pending) {
    return <GapNote>Checking whether this project has moved…</GapNote>;
  }
  if (error !== null) {
    return (
      <GapNote>
        Could not check whether this project has moved: {error.message}
      </GapNote>
    );
  }
  if (moved === null) return null;

  const { artifacts, sessions, tasks, memories } = moved.rows;
  const failure = rebase.error;
  // The daemon refuses a re-base while anything under the old root is still
  // running (`409 WORKSPACE_BUSY`), and this card already has the count.
  const busy = moved.activeTasks > 0;

  return (
    <div className="mt-[12px] rounded-2xl border border-amber-line bg-amber-surface px-[13px] py-[11px]">
      <p className="m-0 text-base leading-[1.6] text-ink">
        This project&rsquo;s store records{" "}
        <span className="font-mono text-2xs-plus">{moved.from}</span>. Its{" "}
        {artifacts} artifacts, {sessions} conversations, {tasks} runs and{" "}
        {memories} memories still point there.
      </p>

      {moved.activeTasks > 0 && (
        <p className="mt-[6px] mb-0 text-base leading-[1.6] text-secondary">
          {moved.activeTasks} run(s) there are still in flight — a re-base is
          refused while a run is in flight; retry once it finishes.
        </p>
      )}

      {failure !== null && (
        <p className="mt-[6px] mb-0 text-base leading-[1.6] text-red-ink">
          {failure.message}
        </p>
      )}

      <div className="mt-[9px] flex flex-wrap items-center gap-[6px]">
        {confirming ? (
          <>
            <span className="text-base text-secondary">
              Re-base all four onto this path?
            </span>
            <Button
              variant="primarySm"
              disabled={rebase.isPending}
              onClick={() => {
                rebase.mutate(
                  { from: moved.from, to: moved.to },
                  {
                    onSuccess: (result) => {
                      setConfirming(false);
                      showToast(
                        `Re-based ${result.moved.artifacts} artifacts, ` +
                          `${result.moved.sessions} conversations, ` +
                          `${result.moved.tasks} runs and ` +
                          `${result.moved.memories} memories`,
                      );
                    },
                  },
                );
              }}
            >
              {rebase.isPending ? "Re-basing…" : "Re-base"}
            </Button>
            <Button variant="ghostSm" onClick={() => setConfirming(false)}>
              Cancel
            </Button>
          </>
        ) : (
          <Button
            variant="outlineRaised"
            disabled={busy}
            onClick={() => setConfirming(true)}
          >
            Re-base to this path
          </Button>
        )}
      </div>
    </div>
  );
}
