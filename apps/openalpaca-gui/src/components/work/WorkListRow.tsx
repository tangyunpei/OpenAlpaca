/**
 * `WorkListRow` / `CompletedRow` (DESIGN_SPEC §3.24) — the Work view's left
 * column.
 *
 * Both rows share one box (`rowStyle`); only the inner layout differs: a live
 * row is top-aligned around a 7px status dot with a two-line block beside it,
 * a completed row is a single centred line ending in a timestamp.
 *
 * The dot is marked decorative because the `StatusLabel` next to it already
 * says the status out loud; on the completed row, which has no label, the row
 * text carries the meaning instead.
 */

import { StatusDot, StatusLabel } from "@/components/ui";
import { cn } from "@/lib/cn";

import type { Run } from "./run-model";

const rowBox = (selected: boolean): string =>
  cn(
    "mb-[2px] block w-full cursor-pointer rounded-xl px-[12px] py-[11px] text-left",
    "transition-[background-color,border-color] duration-[120ms]",
    "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
    selected
      ? "border border-line-popover bg-raised"
      : "border border-transparent bg-transparent hover:bg-sunken",
  );

export interface WorkListRowProps {
  run: Run;
  selected: boolean;
  onSelect: (runId: string) => void;
}

export function WorkListRow({ run, selected, onSelect }: WorkListRowProps) {
  return (
    <button
      type="button"
      aria-current={selected ? "true" : undefined}
      onClick={() => onSelect(run.id)}
      className={rowBox(selected)}
    >
      <span className="flex items-start gap-[8px]">
        <StatusDot
          status={run.status}
          size={7}
          decorative
          className="mt-[5px]"
        />
        <span className="min-w-0 flex-1">
          <span className="block text-md leading-[1.4] font-medium text-pretty text-ink">
            {run.title}
          </span>
          <span className="mt-[5px] flex flex-wrap items-center gap-[8px] font-mono text-2xs-plus text-muted-fg">
            <StatusLabel status={run.status} size="row" />
            {run.meta !== "" && <span>{run.meta}</span>}
          </span>
        </span>
      </span>
    </button>
  );
}

export interface CompletedRowProps {
  run: Run;
  selected: boolean;
  onSelect: (runId: string) => void;
}

export function CompletedRow({ run, selected, onSelect }: CompletedRowProps) {
  return (
    <button
      type="button"
      aria-current={selected ? "true" : undefined}
      onClick={() => onSelect(run.id)}
      className={rowBox(selected)}
    >
      <span className="flex items-center gap-[8px]">
        <span
          aria-hidden
          className="h-[6px] w-[6px] shrink-0 rounded-full bg-disabled"
        />
        <span className="min-w-0 flex-1 truncate text-base text-tertiary">
          {run.title}
        </span>
        {run.stamp !== null && (
          <span className="shrink-0 font-mono text-2xs-plus text-faint">
            {run.stamp}
          </span>
        )}
      </span>
    </button>
  );
}
