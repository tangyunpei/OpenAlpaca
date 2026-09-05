/**
 * `WorkPane` (DESIGN_SPEC §3.18) — the chat aside in work mode.
 *
 * ## Cross-chunk seam
 *
 * This is the only component the chat view imports from the Work chunk, so its
 * surface is deliberately tiny:
 *
 * ```tsx
 * <WorkPane blockedRunId={pendingConfirmationRunId} />
 * ```
 *
 * Everything it needs is either server state (`GET /v1/tasks`) or UI state (the
 * zustand store) and it reads both itself. The single exception is
 * `blockedRunId`: only the chat view knows which run holds a pending tool
 * confirmation, and that flag drives §4.4's red→green coupling — the run card's
 * note dot and its `block` lane bars turn green the moment the confirmation is
 * answered.
 *
 * The aside's own chrome — width, `border-left`, background, the resizer — is
 * the chat layout's business (§2.2). `WorkPane` fills the column it is handed:
 * mount it inside a `flex-col` parent and it supplies the 46px header and the
 * scrolling body, nothing else.
 *
 * Body membership is §3.18's: every run whose status is not `done`.
 */

import { useMemo } from "react";

import { PaneHeader } from "@/components/ui";
import { useTasks } from "@/hooks/useTasks";
import { useTaskTimeline } from "@/hooks/useUnbacked";
import { Button } from "@/components/ui";
import { cn } from "@/lib/cn";
import { GAPS, gapNote } from "@/lib/unavailable";
import { useUiStore } from "@/stores/ui";

import { RunCard } from "./RunCard";
import {
  partitionRuns,
  toRun,
  type OutcomeArtifact,
  type Run,
} from "./run-model";
import { useRunController, type RunController } from "./useRunController";

/** Enough rows to fill the pane and the "done" counter without paging. */
const RUN_WINDOW = 60;

/** Shown under a card's file rows when they cannot be opened (GAP-04). */
const FILES_NOTE = gapNote(GAPS["GAP-04"]);

export interface WorkPaneProps {
  /**
   * The run holding a pending tool confirmation, or `null`. Chat-owned state:
   * it arrives on the SSE stream, which this pane does not read.
   */
  blockedRunId?: string | null;
  /**
   * A confirmation is pending in the lane but the run behind it is unknown.
   * Accepted so `<WorkPane {...props} />` satisfies the chat view's
   * `WorkPaneSlotProps`; with no run id there is nothing to mark, and marking
   * an arbitrary card would be a guess.
   */
  blocked?: boolean;
  /** `Full view`. Defaults to switching the app to the Work view. */
  onFullView?: () => void;
  /** `›`. Defaults to collapsing the aside. */
  onCollapse?: () => void;
  className?: string;
}

export function WorkPane({
  blockedRunId = null,
  blocked,
  onFullView,
  onCollapse,
  className,
}: WorkPaneProps) {
  const dense = useUiStore((s) => s.dense);
  const setView = useUiStore((s) => s.setView);
  const closeWorkPane = useUiStore((s) => s.closeWorkPane);
  const openSidePanel = useUiStore((s) => s.openSidePanel);
  const controller = useRunController();

  const query = useTasks({ limit: RUN_WINDOW });
  const tasks = query.data;
  const partition = useMemo(() => {
    const now = new Date();
    return partitionRuns(
      (tasks ?? []).map((task) => toRun(task, now)),
      now,
    );
  }, [tasks]);

  // `blocked` alone cannot mark a card: without the run id it names no run.
  void blocked;

  const openFile = (artifact: OutcomeArtifact): void => {
    if (artifact.id !== null) openSidePanel(artifact.id);
  };

  return (
    <div className={cn("flex min-h-0 flex-1 flex-col", className)}>
      <PaneHeader
        variant="aside"
        title="Work"
        meta={`${partition.activeCount} active · ${partition.doneCount} done`}
      >
        <Button
          variant="ghost2xs"
          onClick={onFullView ?? (() => setView("work"))}
        >
          Full view
        </Button>
        <Button
          variant="iconGlyph"
          aria-label="Collapse work pane"
          onClick={onCollapse ?? closeWorkPane}
        >
          ›
        </Button>
      </PaneHeader>

      <div className="sc min-h-0 flex-1 overflow-y-auto p-[14px]">
        {query.isPending ? (
          <p className="m-0 font-mono text-2xs-plus text-faint">
            Loading runs…
          </p>
        ) : query.isError ? (
          <p className="m-0 text-sm-plus leading-[1.5] text-red-ink">
            Could not reach the daemon.
          </p>
        ) : partition.live.length === 0 ? (
          <p className="m-0 text-sm-plus leading-[1.5] text-muted-fg">
            Nothing running. Ask for something in the chat and it lands here.
          </p>
        ) : (
          partition.live.map((run) => (
            <RunCardSlot
              key={run.id}
              run={run}
              dense={dense}
              blocked={blockedRunId !== null && blockedRunId === run.id}
              controller={controller}
              onOpenFile={openFile}
            />
          ))
        )}
      </div>
    </div>
  );
}

interface RunCardSlotProps {
  run: Run;
  dense: boolean;
  blocked: boolean;
  controller: RunController;
  onOpenFile: (artifact: OutcomeArtifact) => void;
}

/**
 * One card, so the per-run timeline hook has a component to live in. It is
 * `unavailable` for every run today (GAP-09), but as a hook it becomes a real
 * per-run query the moment the route exists — no restructuring.
 */
function RunCardSlot({
  run,
  dense,
  blocked,
  controller,
  onOpenFile,
}: RunCardSlotProps) {
  const timeline = useTaskTimeline(run.id);
  const unopenable =
    run.artifactCount > run.artifacts.length ||
    run.artifacts.some((artifact) => artifact.id === null);

  return (
    <RunCard
      run={run}
      timeline={timeline}
      dense={dense}
      blocked={blocked}
      busy={controller.busyFor(run.id)}
      onAction={controller.perform}
      onOpenFile={(artifact) => onOpenFile(artifact)}
      onAllFiles={controller.openRunFiles}
      filesNote={unopenable ? FILES_NOTE : null}
    />
  );
}
