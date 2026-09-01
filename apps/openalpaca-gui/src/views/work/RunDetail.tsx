/**
 * The Work view's right column (DESIGN_SPEC §3.26, §5.2).
 *
 * Header, action group (or the terminal banner), then three `SectionCard`s.
 * The whole column scrolls as one — it has no sticky header, unlike the
 * Library's (§2.3).
 *
 * Two honest absences in the header:
 *   * `$0.41` per run — nothing serves per-run cost (GAP-08), so the meta row
 *     carries a `cost —` cell whose tooltip names the route that would fill it
 *     rather than an invented figure.
 *   * the run's own detail is refetched by id, because live `task_status`
 *     events arrive with `title: ""` (GAP-07).
 */

import { useMemo } from "react";

import { StatusLabel } from "@/components/ui";
import {
  RunActionBar,
  TerminalBanner,
  UnavailableActionsNote,
} from "@/components/work/RunActionBar";
import {
  gapTooltip,
  liveRunActions,
  terminalRunActions,
  type RunActionId,
} from "@/components/work/run-actions";
import { runEventsFromRing } from "@/components/work/run-events";
import { toRun, type Run } from "@/components/work/run-model";
import { useEventRing } from "@/hooks/useDaemonEvents";
import { useTask } from "@/hooks/useTasks";
import { useTaskTimeline } from "@/hooks/useUnbacked";
import { isLive } from "@/components/ui";

import { EventLogSection } from "./EventLogSection";
import { OutputSection } from "./OutputSection";
import { TimelineSection, type RunAssignment } from "./TimelineSection";

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
  const detail = useTask(runId);
  const timeline = useTaskTimeline(runId);
  const ring = useEventRing();

  const task = detail.data?.task;
  const run = useMemo<Run | null>(
    () => (task === undefined ? fallbackRun : toRun(task)),
    [task, fallbackRun],
  );

  const assignments = useMemo<RunAssignment[]>(
    () => detail.data?.assignments ?? [],
    [detail.data],
  );

  const events = useMemo(
    () => (runId === null ? [] : runEventsFromRing(ring, runId)),
    [ring, runId],
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
        <span title={gapTooltip("GAP-08")} className="text-faint">
          cost —
        </span>
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
        timeline={timeline}
        assignments={assignments}
        blocked={blocked}
      />
      <OutputSection artifacts={run.artifacts} count={run.artifactCount} />
      <EventLogSection events={events} />
    </div>
  );
}
