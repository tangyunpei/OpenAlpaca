/**
 * `RunCard` (DESIGN_SPEC §3.19) — the aside's unit of work.
 *
 * Presentational on purpose: every action is a callback, so the same card
 * serves the chat aside (via `WorkPane`) and any future surface, and its status
 * → visual mapping is testable without a query client.
 *
 * The `Files` block reads `task.outcome.artifacts`, which is free-form JSON the
 * dispatcher happens to write — there is no artifact resource (GAP-04). Rows
 * whose entry carries no id cannot be opened, so they render as plain rows and
 * the block carries the note naming the missing API rather than pretending the
 * click does something.
 */

import {
  Button,
  Eyebrow,
  FileBadge,
  languageFromName,
  StatusDot,
  StatusLabel,
} from "@/components/ui";
import { cn } from "@/lib/cn";
import type { TaskTimeline } from "@/lib/api/unbacked";
import type { Availability } from "@/lib/unavailable";

import { ParallelWorkBlock } from "./ParallelWork";
import { RunActionBar, TerminalRunRow } from "./RunActionBar";
import {
  liveRunActions,
  terminalRunActions,
  type RunActionId,
} from "./run-actions";
import {
  isRaisedRun,
  isTerminalRun,
  type OutcomeArtifact,
  type Run,
} from "./run-model";

/** The design shows at most four file rows before the overflow link. */
const MAX_FILE_ROWS = 4;

export interface RunCardProps {
  run: Run;
  /** GAP-09 today; the `available` branch is drawn all the same. */
  timeline: Availability<TaskTimeline>;
  /** This run holds the pending tool confirmation. */
  blocked?: boolean;
  dense?: boolean;
  busy?: RunActionId | null;
  onAction: (action: RunActionId, run: Run) => void;
  /** Opens the aside's file panel; omitted when the artifact has no id. */
  onOpenFile?: (artifact: OutcomeArtifact, run: Run) => void;
  onAllFiles?: (run: Run) => void;
  /** Muted line under the file rows — the reason they cannot be opened. */
  filesNote?: string | null;
  className?: string;
}

export function RunCard({
  run,
  timeline,
  blocked = false,
  dense = false,
  busy = null,
  onAction,
  onOpenFile,
  onAllFiles,
  filesNote,
  className,
}: RunCardProps) {
  const raised = isRaisedRun(run.status);
  const terminal = isTerminalRun(run.status);
  const actions = terminal ? terminalRunActions() : liveRunActions(run.status);
  const rerun = actions.find((action) => action.id === "rerun");
  const visible = run.artifacts.slice(0, MAX_FILE_ROWS);
  const overflow = Math.max(0, run.artifactCount - visible.length);

  return (
    <article
      className={cn(
        "mb-[12px] overflow-hidden rounded-3xl border",
        raised
          ? "border-line-strong bg-raised shadow-card-active"
          : "border-line bg-inactive",
        className,
      )}
    >
      <div
        className={cn(
          dense
            ? "px-[13px] pt-[11px] pb-[10px]"
            : "px-[15px] pt-[14px] pb-[12px]",
        )}
      >
        <div className="flex items-start gap-[8px]">
          <StatusDot
            status={run.status}
            size={7}
            decorative
            className="mt-[5px]"
          />
          <div className="min-w-0 flex-1">
            <h3 className="m-0 text-md-plus leading-[1.4] font-semibold tracking-snug text-pretty text-ink">
              {run.title}
            </h3>
            <div className="mt-[6px] flex flex-wrap items-center gap-[8px] font-mono text-xs text-muted-fg">
              <StatusLabel status={run.status} size="card" />
              {run.meta !== "" && <span>{run.meta}</span>}
            </div>
          </div>
        </div>

        {raised && (
          <>
            <ParallelWorkBlock timeline={timeline} blocked={blocked} />
            {run.note !== null && (
              <div className="mt-[10px] flex items-start gap-[7px]">
                <span
                  aria-hidden
                  className={cn(
                    "mt-[3px] h-[6px] w-[6px] shrink-0 rounded-full",
                    blocked ? "bg-red" : "bg-green",
                  )}
                />
                <p className="m-0 text-sm-plus leading-[1.45] text-tertiary">
                  {run.note}
                </p>
              </div>
            )}
          </>
        )}

        {(visible.length > 0 || run.artifactCount > 0) && (
          <div className="mt-[12px] border-t border-line-hair pt-[11px]">
            <Eyebrow tone="faint" className="mb-[7px]">
              {`Files · ${run.artifactCount}`}
            </Eyebrow>

            <div className="flex flex-col gap-[4px]">
              {visible.map((artifact) => (
                <FileRow
                  key={artifact.id ?? artifact.name}
                  artifact={artifact}
                  onOpen={
                    artifact.id !== null && onOpenFile !== undefined
                      ? () => onOpenFile(artifact, run)
                      : undefined
                  }
                />
              ))}
            </div>

            {overflow > 0 && onAllFiles !== undefined && (
              <Button
                variant="bareLink"
                className="mt-[2px] px-[9px] py-[3px] text-left"
                onClick={() => onAllFiles(run)}
              >
                {`+ ${overflow} more in Library ↗`}
              </Button>
            )}

            {filesNote !== null && filesNote !== undefined && (
              <p className="mt-[6px] mb-0 font-mono text-2xs-plus leading-[1.5] text-faint">
                {filesNote}
              </p>
            )}
          </div>
        )}
      </div>

      {terminal && rerun !== undefined ? (
        <TerminalRunRow
          note={run.note}
          status={
            run.status === "done"
              ? "done"
              : run.status === "failed"
                ? "failed"
                : "cancelled"
          }
          rerun={rerun}
          dense={dense}
          onAction={(action) => onAction(action, run)}
        />
      ) : (
        <RunActionBar
          actions={actions}
          size="card"
          dense={dense}
          busy={busy}
          onAction={(action) => onAction(action, run)}
        />
      )}
    </article>
  );
}

interface FileRowProps {
  artifact: OutcomeArtifact;
  /** Absent when the entry has no id — the row then states, not acts. */
  onOpen?: () => void;
}

function FileRow({ artifact, onOpen }: FileRowProps) {
  const inner = (
    <>
      <FileBadge
        kind={artifact.kind}
        size={17}
        language={languageFromName(artifact.name)}
      />
      <span className="min-w-0 flex-1 truncate text-sm-plus text-ink">
        {artifact.name}
      </span>
      {artifact.stamp !== null && (
        <span className="shrink-0 font-mono text-2xs text-faint">
          {artifact.stamp}
        </span>
      )}
    </>
  );

  const box =
    "flex w-full items-center gap-[8px] rounded-md border border-line-hair-2 bg-sunken px-[9px] py-[6px] text-left";

  if (onOpen === undefined) {
    return <div className={box}>{inner}</div>;
  }
  return (
    <button
      type="button"
      onClick={onOpen}
      className={cn(
        box,
        "cursor-pointer transition-[background-color,border-color] duration-[120ms]",
        "hover:border-line-strong hover:bg-raised",
        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
      )}
    >
      {inner}
    </button>
  );
}
