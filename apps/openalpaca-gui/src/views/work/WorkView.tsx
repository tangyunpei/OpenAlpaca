/**
 * The Work view (DESIGN_SPEC §2.3, §5.2) — a resizable run list beside a run
 * detail. The detail column has no sticky header: the whole column scrolls as
 * one, which is what distinguishes it from the Library's split.
 *
 * The list is one `GET /v1/tasks` window, partitioned client-side (§4.2). Live
 * `task_status` frames invalidate the query through `QueryProvider`, so the
 * list follows the daemon without polling.
 *
 * Selection falls back to the first live run so the detail column is never
 * empty while runs exist; the store keeps the choice across view switches.
 */

import { useMemo } from "react";

import { Resizer } from "@/components/shell";
import { PaneHeader } from "@/components/ui";
import { partitionRuns, toRun } from "@/components/work/run-model";
import { useRunController } from "@/components/work/useRunController";
import { useTasks } from "@/hooks/useTasks";
import { useUiStore } from "@/stores/ui";

import { RunDetail } from "./RunDetail";
import { WorkList } from "./WorkList";

/** One window of recent runs — `list_recent` returns every status. */
const RUN_WINDOW = 60;

export interface WorkViewProps {
  /** The run holding a pending tool confirmation; chat-owned state. */
  blockedRunId?: string | null;
}

export default function WorkView({ blockedRunId = null }: WorkViewProps) {
  const listWidth = useUiStore((s) => s.paneWidths.workListW);
  const selectedRunId = useUiStore((s) => s.selectedRunId);
  const focusRun = useUiStore((s) => s.focusRun);
  const controller = useRunController();

  const query = useTasks({ limit: RUN_WINDOW });
  const tasks = query.data;

  const { runs, partition } = useMemo(() => {
    const now = new Date();
    const mapped = (tasks ?? []).map((task) => toRun(task, now));
    return { runs: mapped, partition: partitionRuns(mapped, now) };
  }, [tasks]);

  const selected =
    selectedRunId ??
    partition.live[0]?.id ??
    partition.completedToday[0]?.id ??
    null;
  const selectedRun = runs.find((run) => run.id === selected) ?? null;

  return (
    <section className="flex min-w-0 flex-1 bg-main">
      <div
        className="flex shrink-0 flex-col border-r border-line-subtle"
        style={{ width: listWidth }}
      >
        <PaneHeader
          title="Work"
          meta={`${partition.activeCount} active · ${partition.doneCount} done`}
        />
        <div className="sc min-h-0 flex-1 overflow-y-auto p-[10px]">
          <WorkList
            live={partition.live}
            completedToday={partition.completedToday}
            selectedRunId={selected}
            onSelect={focusRun}
            isPending={query.isPending}
            isError={query.isError}
          />
        </div>
      </div>

      <Resizer paneKey="workListW" direction={1} label="the run list" />

      <div className="sc min-w-0 flex-1 overflow-y-auto">
        <div className="max-w-detail-max px-[30px] pt-[24px] pb-[30px]">
          <RunDetail
            runId={selected}
            fallbackRun={selectedRun}
            blockedRunId={blockedRunId}
            busy={selected === null ? null : controller.busyFor(selected)}
            onAction={controller.perform}
          />
        </div>
      </div>
    </section>
  );
}
