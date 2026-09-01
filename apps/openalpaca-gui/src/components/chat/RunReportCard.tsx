/**
 * `RunReportCard` (DESIGN_SPEC §3.12) — a finished background workflow
 * reported into the transcript.
 *
 * Every value on the card is something the daemon sent: the id and title come
 * from the task, the duration from `created_at → completed_at`, the artifact
 * chips from the run outcome's own artifact refs. Cost has no source at all
 * (GAP-08 — `daily_cost_usd` is hardcoded and there is no per-task cost), so
 * the `$` segment is **omitted** rather than printed as `$0.00`; `note` is
 * where the card says why something is missing.
 */

import { FileBadge, type FileKind } from "@/components/ui";
import { cn } from "@/lib/cn";

export type RunReportStatus = "done" | "failed" | "cancelled";

export interface ArtifactChip {
  id: string;
  name: string;
  kind: FileKind;
  language?: string | null;
}

export interface RunReportCardProps {
  status: RunReportStatus;
  /** `13:41`, or `null` when the task carries no usable completion stamp. */
  time: string | null;
  runId: string;
  /** `6m 12s`, or `null` when either end of the wall clock is missing. */
  duration: string | null;
  title: string;
  summary: string | null;
  chips?: readonly ArtifactChip[];
  onOpenChip?: (chipId: string) => void;
  /** e.g. "Artifact API not yet available" — names a gap, never fakes a row. */
  note?: string | null;
}

const EYEBROW: Record<RunReportStatus, string> = {
  done: "Run finished",
  failed: "Run failed",
  cancelled: "Run cancelled",
};

const DOT: Record<RunReportStatus, string> = {
  done: "bg-green",
  failed: "bg-red",
  cancelled: "bg-muted-fg",
};

const EYEBROW_COLOR: Record<RunReportStatus, string> = {
  done: "text-green",
  failed: "text-red",
  cancelled: "text-muted-fg",
};

export function RunReportCard({
  status,
  time,
  runId,
  duration,
  title,
  summary,
  chips = [],
  onOpenChip,
  note = null,
}: RunReportCardProps) {
  const right = [runId, duration].filter((part) => part !== null).join(" · ");

  return (
    <section className="mb-[26px] overflow-hidden rounded-3xl border border-line bg-raised">
      <header className="flex items-center gap-[9px] border-b border-line-hair bg-sunken px-[14px] py-[10px]">
        <span
          aria-hidden
          className={cn(
            "block h-[6px] w-[6px] shrink-0 rounded-full",
            DOT[status],
          )}
        />
        <span
          className={cn(
            "font-mono text-2xs-plus tracking-eyebrow uppercase",
            EYEBROW_COLOR[status],
          )}
        >
          {EYEBROW[status]}
          {time !== null && ` · ${time}`}
        </span>
        <span className="ml-auto font-mono text-xs text-faint">{right}</span>
      </header>

      <div className="px-[14px] py-[13px]">
        <p className="mb-[6px] text-md-plus font-semibold [text-wrap:pretty]">
          {title}
        </p>
        {summary !== null && summary !== "" && (
          <p className="mt-0 mb-[11px] text-md-plus leading-[1.6] [text-wrap:pretty] text-secondary">
            {summary}
          </p>
        )}

        {chips.length > 0 && (
          <div className="flex flex-wrap gap-[6px]">
            {chips.map((chip, index) => (
              <button
                key={chip.id}
                type="button"
                onClick={() => onOpenChip?.(chip.id)}
                className={cn(
                  "flex cursor-pointer items-center gap-[7px] rounded-md border border-line px-[9px] py-[5px] text-sm-plus",
                  "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
                  index === 0
                    ? "bg-muted text-ink hover:bg-muted-active"
                    : "bg-transparent text-secondary hover:bg-muted",
                )}
              >
                <FileBadge
                  kind={chip.kind}
                  size={14}
                  language={chip.language ?? null}
                />
                {chip.name}
              </button>
            ))}
          </div>
        )}

        {note !== null && note !== "" && (
          <p className="mt-[9px] mb-0 font-mono text-2xs-plus text-faint">
            {note}
          </p>
        )}
      </div>
    </section>
  );
}
