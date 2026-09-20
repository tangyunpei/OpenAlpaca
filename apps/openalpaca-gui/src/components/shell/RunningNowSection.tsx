/**
 * `RunningNowSection` (DESIGN_SPEC §3.4).
 *
 * Membership is every run that is not finished — which is exactly what
 * `GET /v1/tasks?status=active` returns (`queued`, `running`, `paused`).
 *
 * Two honest departures from the mock:
 *   * The design shows a hand-written `short` title ("Connector audit"). A
 *     `Task` has one `title` and nothing shorter, so the full title is rendered
 *     and truncated by the row's own ellipsis rather than invented.
 *   * The trailing `wait` marker needs to know which run holds the pending tool
 *     confirmation. That lives in the chat stream, so it arrives as
 *     `blockedRunId` from above rather than being guessed here.
 *
 * The section hides itself when nothing is live: an eyebrow over an empty list
 * is noise, and the rail has no empty-state copy in the design.
 */

import { Eyebrow, StatusDot, type UiStatus } from "@/components/ui";

export interface RailRun {
  id: string;
  title: string;
  status: UiStatus;
}

export interface RunningNowSectionProps {
  runs: readonly RailRun[];
  onFocusRun: (runId: string) => void;
  /** The run holding a pending tool confirmation, if any. */
  blockedRunId?: string | null;
}

export function RunningNowSection({
  runs,
  onFocusRun,
  blockedRunId = null,
}: RunningNowSectionProps) {
  if (runs.length === 0) return null;

  return (
    <div className="mt-[22px] px-[6px]">
      <Eyebrow className="mb-[8px]">Running now</Eyebrow>
      <ul className="m-0 flex list-none flex-col gap-[8px] p-0">
        {runs.map((run) => (
          <li key={run.id}>
            <button
              type="button"
              onClick={() => onFocusRun(run.id)}
              className="flex w-full cursor-pointer items-center gap-[7px] border-none bg-transparent p-0 text-left font-sans hover:opacity-65 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
            >
              <StatusDot status={run.status} size={6} />
              <span className="flex-1 truncate text-sm-plus leading-[1.35] text-body">
                {run.title}
              </span>
              {blockedRunId === run.id && (
                <span className="font-mono text-[8.5px] text-red">wait</span>
              )}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
