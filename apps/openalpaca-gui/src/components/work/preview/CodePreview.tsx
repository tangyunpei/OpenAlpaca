/**
 * Code renderer (DESIGN_SPEC §3.25b).
 *
 * The design draws a *diff-annotated* file: a header carrying the path and the
 * `+41 −6` counters, then lines that may be added, removed or context. Both
 * shapes come from `diff.ts`, so a plain source file (every line context) and a
 * patch use one component.
 *
 * Compact has no gutter at all; full has a 44px one whose numbers take the
 * softened green/red of their line state.
 */

import { cn } from "@/lib/cn";

import type { DiffLine } from "../diff";

import { DiffCounters, PreviewShell, PreviewStrip } from "./PreviewShell";
import type { PreviewSize } from "./types";

/** §3.25b line states — shared by the code and diff renderers. */
export const LINE_TONE: Record<DiffLine["kind"], string> = {
  added: "bg-green-diff text-green-ink",
  removed: "bg-red-diff text-red-ink",
  context: "text-body",
  hunk: "text-muted-fg",
  meta: "text-muted-fg",
};

/** Gutter numbers soften rather than repeat the line colour. */
const GUTTER_TONE: Record<DiffLine["kind"], string> = {
  // Not palette tokens: §3.25b names these two colours only here.
  added: "text-[#7EA98F]",
  removed: "text-[#C99A8C]",
  context: "text-gutter",
  hunk: "text-gutter",
  meta: "text-gutter",
};

export interface CodePreviewProps {
  path: string;
  lines: readonly DiffLine[];
  size: PreviewSize;
  addedLines?: number | null;
  removedLines?: number | null;
  className?: string;
}

export function CodePreview({
  path,
  lines,
  size,
  addedLines,
  removedLines,
  className,
}: CodePreviewProps) {
  const full = size === "full";

  return (
    <PreviewShell size={size} className={className}>
      <PreviewStrip size={size}>
        <span className="min-w-0 flex-1 truncate">{path}</span>
        <DiffCounters
          added={addedLines}
          removed={removedLines}
          className={full ? "ml-auto" : undefined}
        />
      </PreviewStrip>

      <div
        className={cn(
          "overflow-x-auto font-mono leading-[1.85]",
          full ? "max-w-[760px] text-sm-plus" : "py-[6px] text-xs-plus",
        )}
      >
        {lines.map((line, index) => {
          const number = line.newNumber ?? line.oldNumber;
          return (
            <div
              // Line order is the identity here; a patch has no stable line id.
              key={index}
              className={cn(
                LINE_TONE[line.kind],
                full ? "flex" : "px-[11px] whitespace-pre",
              )}
            >
              {full && (
                <span
                  aria-hidden
                  className={cn(
                    "w-[44px] shrink-0 pr-[12px] text-right",
                    GUTTER_TONE[line.kind],
                  )}
                >
                  {number ?? ""}
                </span>
              )}
              <span
                className={cn(full && "flex-1 pr-[14px] whitespace-pre-wrap")}
              >
                {line.text === "" ? " " : line.text}
              </span>
            </div>
          );
        })}
      </div>
    </PreviewShell>
  );
}
