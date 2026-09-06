/**
 * The Work view's right column (DESIGN_SPEC §3.26, §5.2).
 *
 * Header, action group (or the terminal banner), then three `SectionCard`s.
 * The whole column scrolls as one — it has no sticky header, unlike the
 * Library's (§2.3).
 *
 * One honest absence in the header: `GET /v1/tasks/{id}` (this view's own
 * fetch) has no `cost_usd` field — only the list route does (GAP-08b landed
 * there, not here; the two shapes unify in a later phase). `run` therefore
 * prefers the detail's own cost when it has one and falls back to the list
 * row's (`fallbackRun`) otherwise, so the figure shown while browsing from
 * the Work list does not vanish once the detail request resolves; it reads
 * `cost —` only when neither source has one (e.g. opened outside the list's
 * fetched window).
 */

import { useMemo } from "react";

import { StatusLabel } from "@/components/ui";
import {
  RunActionBar,
  TerminalBanner,
  UnavailableActionsNote,
} from "@/components/work/RunActionBar";
import {
  liveRunActions,
  terminalRunActions,
  type RunActionId,
} from "@/components/work/run-actions";
import {
  toRun,
  type OutcomeArtifact,
  type Run,
} from "@/components/work/run-model";
import { toFileKind } from "@/components/ui";
import { useArtifacts } from "@/hooks/useArtifacts";
import { useRunEventLog } from "@/hooks/useEventHistory";
import { useTask, useTaskTimeline } from "@/hooks/useTasks";
import { isLive } from "@/components/ui";
import { useUiStore } from "@/stores/ui";

import { formatClock } from "@/components/work/run-model";

import { EventLogSection } from "./EventLogSection";
import { OutputSection } from "./OutputSection";
import { TimelineSection } from "./TimelineSection";

export interface RunDetailProps {
  runId: string | null;
  /** The list row for this run, shown until the detail request resolves. */
  fallbackRun?: Run | null;
  blockedRunId?: string | null;
  busy?: RunActionId | null;
  onAction: (action: RunActionId, run: Run) => void;
}

export function RunDetail({
  runId,
  fallbackRun = null,
  blockedRunId = null,
  busy = null,
  onAction,
}: RunDetailProps) {
  const openSidePanel = useUiStore((state) => state.openSidePanel);
  const detail = useTask(runId);
  const timeline = useTaskTimeline(runId);
  const eventLog = useRunEventLog(runId);

  const task = detail.data?.task;
  const run = useMemo<Run | null>(() => {
    if (task === undefined) return fallbackRun;
    const detailRun = toRun(task);
    const costUsd = fallbackRun?.costUsd ?? null;
    if (detailRun.costUsd !== null || costUsd === null) return detailRun;
    // The detail route carries no cost_usd; fold in the list row's figure
    // (already fetched to get here) rather than letting it disappear once
    // the detail request resolves.
    const meta = [detailRun.meta, `$${costUsd.toFixed(2)}`]
      .filter((segment) => segment !== "")
      .join(" · ");
    return { ...detailRun, costUsd, meta };
  }, [task, fallbackRun]);

  // The design draws up to six rows (§5.2); the query fetches a wider page so
  // the dropped `dag_node_status` duplicates cannot empty the card.
  const events = useMemo(
    () => (eventLog.data?.events ?? []).slice(0, 6),
    [eventLog.data],
  );

  // The run's own files, with ids that open. `run.outcome.artifacts` stays the
  // fallback for a run the list cannot answer for.
  const files = useArtifacts(
    { taskId: runId ?? "" },
    { enabled: runId !== null },
  );
  const outputs = useMemo<OutcomeArtifact[] | null>(
    () =>
      files.data === undefined
        ? null
        : files.data.artifacts.map((artifact) => ({
            id: artifact.id,
            name: artifact.name,
            kind: toFileKind(artifact.kind),
            stamp: formatClock(artifact.updated_at),
          })),
    [files.data],
  );

  if (runId === null || run === null) {
    return (
      <p className="m-0 text-md text-muted-fg">
        {detail.isError
          ? "Could not load this run."
          : "Select a run to see what it did."}
      </p>
    );
  }

  const live = isLive(run.status);
  const actions = live ? liveRunActions(run.status) : terminalRunActions();
  const blocked = blockedRunId === run.id;

  return (
    <div>
      <h2 className="m-0 text-4xl leading-[1.3] font-semibold tracking-tightest text-pretty text-ink">
        {run.title}
      </h2>

      <div className="mt-[8px] flex flex-wrap items-center gap-[10px] font-mono text-xs-plus text-muted-fg">
        <StatusLabel status={run.status} size="detail" />
        <span>{run.id.slice(0, 8)}</span>
        {run.meta !== "" && <span>{run.meta}</span>}
        {run.started !== null && <span>{`started ${run.started}`}</span>}
        {run.costUsd === null && (
          <span
            title="This run is outside the list's fetched window, which is the only route that serves cost"
            className="text-faint"
          >
            cost —
          </span>
        )}
      </div>

      {live ? (
        <>
          <RunActionBar
            actions={actions}
            size="detail"
            busy={busy}
            onAction={(action) => onAction(action, run)}
          />
          <UnavailableActionsNote actions={actions} />
        </>
      ) : (
        <>
          <TerminalBanner
            status={
              run.status === "done"
                ? "done"
                : run.status === "failed"
                  ? "failed"
                  : "cancelled"
            }
            note={run.note}
            actions={actions}
            onAction={(action) => onAction(action, run)}
          />
          <UnavailableActionsNote actions={actions} />
        </>
      )}

      <TimelineSection
        timeline={timeline.data ?? null}
        error={timeline.error !== null ? timeline.error.message : null}
        blocked={blocked}
      />
      <OutputSection
        artifacts={outputs ?? run.artifacts}
        count={run.artifactCount}
        onOpen={(artifact) => {
          if (artifact.id !== null) openSidePanel(artifact.id);
        }}
        note={
          files.error !== null
            ? `The run's files could not be listed — ${files.error.message}`
            : null
        }
      />
      <EventLogSection
        events={events}
        error={eventLog.error !== null ? eventLog.error.message : null}
      />
    </div>
  );
}
