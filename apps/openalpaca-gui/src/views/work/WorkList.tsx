/**
 * The Work view's left column (DESIGN_SPEC §2.3, §3.24, §5.2).
 *
 * Live runs first — which per §4.2 means every status except `done`, so a
 * cancelled or failed run stays in the main list — then a `Completed today`
 * divider and the compact completed rows. The divider is hidden when nothing
 * finished today; the design has no zero-state for it.
 */

import { Eyebrow } from "@/components/ui";
import { CompletedRow, WorkListRow } from "@/components/work/WorkListRow";
import type { Run } from "@/components/work/run-model";

export interface WorkListProps {
  live: readonly Run[];
  completedToday: readonly Run[];
  selectedRunId: string | null;
  onSelect: (runId: string) => void;
  isPending: boolean;
  isError: boolean;
}

export function WorkList({
  live,
  completedToday,
  selectedRunId,
  onSelect,
  isPending,
  isError,
}: WorkListProps) {
  if (isPending) {
    return (
      <p className="m-0 px-[6px] font-mono text-2xs-plus text-faint">
        Loading runs…
      </p>
    );
  }
  if (isError) {
    return (
      <p className="m-0 px-[6px] text-sm-plus leading-[1.5] text-red-ink">
        Could not reach the daemon.
      </p>
    );
  }
  if (live.length === 0 && completedToday.length === 0) {
    return (
      <p className="m-0 px-[6px] text-sm-plus leading-[1.5] text-muted-fg">
        No runs yet. Background work started from the chat shows up here.
      </p>
    );
  }

  return (
    <div>
      {live.map((run) => (
        <WorkListRow
          key={run.id}
          run={run}
          selected={run.id === selectedRunId}
          onSelect={onSelect}
        />
      ))}

      {completedToday.length > 0 && (
        <>
          <Eyebrow tone="faint" className="mt-[16px] mb-[8px] px-[6px]">
            Completed today
          </Eyebrow>
          {completedToday.map((run) => (
            <CompletedRow
              key={run.id}
              run={run}
              selected={run.id === selectedRunId}
              onSelect={onSelect}
            />
          ))}
        </>
      )}
    </div>
  );
}
