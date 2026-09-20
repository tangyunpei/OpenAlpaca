/**
 * `DiffTab` (DESIGN_SPEC §3.25 `DiffTab`) — the standalone v1 → v2 view, in
 * both sizes.
 *
 * The `+9 / −2` counters are **counted from the patch**, not read off the
 * response: a counter that disagrees with the lines under it is worse than no
 * counter, and the proposed endpoint's `added_lines` is a convenience field.
 *
 * `ArtifactDiffTab` is the container. `GET …/diff` answers 200 with a real
 * unified patch, or refuses with a 409 it names (`NOT_DIFFABLE` for an image or
 * binary, `DIFF_TOO_LARGE` past the 8 MiB cap); a single-version artifact has
 * no pair to ask about at all. All three arrive here as `diff = null` plus the
 * sentence explaining which one it was — an empty diff pane would read as
 * "nothing changed", which is a different claim.
 */

import { useMemo } from "react";

import { cn } from "@/lib/cn";
import type { ArtifactDiff } from "@/lib/api/artifacts";

import { parseUnifiedDiff, type DiffLine } from "./diff";
import { PreviewShell, PreviewUnavailable } from "./preview";
import type { PreviewSize } from "./preview";

const LINE_TONE: Record<DiffLine["kind"], string> = {
  added: "bg-green-diff text-green-ink",
  removed: "bg-red-diff text-red-ink",
  context: "text-muted-fg",
  hunk: "text-faint",
  meta: "text-faint",
};

/** The `+`/`-` marker is re-attached so a copied diff stays a valid patch. */
function marker(kind: DiffLine["kind"]): string {
  if (kind === "added") return "+";
  if (kind === "removed") return "−";
  return " ";
}

export interface DiffViewProps {
  patch: string;
  size: PreviewSize;
  /** `v1` / `v2` — the header's version labels. */
  fromLabel?: string;
  toLabel?: string;
  /** Times shown after the version pair, when known. */
  fromTime?: string | null;
  toTime?: string | null;
  className?: string;
}

export function DiffView({
  patch,
  size,
  fromLabel = "v1",
  toLabel = "v2",
  fromTime,
  toTime,
  className,
}: DiffViewProps) {
  const full = size === "full";
  const parsed = useMemo(() => parseUnifiedDiff(patch), [patch]);
  const times = [fromTime, toTime].filter(
    (time): time is string =>
      time !== null && time !== undefined && time !== "",
  );

  return (
    <PreviewShell size={size} className={className}>
      <div
        className={cn(
          "flex items-center border-b border-line-hair bg-sunken font-mono text-tertiary",
          full
            ? "gap-[10px] px-[14px] py-[10px] text-xs-plus"
            : "gap-[8px] px-[11px] py-[8px] text-2xs-plus",
        )}
      >
        <span>{`${fromLabel} → ${toLabel}`}</span>
        {times.length > 0 && (
          <span className="text-faint">{times.join(" · ")}</span>
        )}
        <span className="ml-auto flex shrink-0 items-center gap-[8px]">
          <span className="text-green">{`+${parsed.added}`}</span>
          <span className="text-red">{`−${parsed.removed}`}</span>
        </span>
      </div>

      <div
        className={cn(
          "overflow-x-auto font-mono",
          full
            ? "py-[6px] text-sm-plus leading-[1.9]"
            : "py-[5px] text-xs-plus leading-[1.85]",
        )}
      >
        {parsed.lines.map((line, index) => (
          <div
            // A patch line has no id; its position is its identity.
            key={index}
            className={cn(
              LINE_TONE[line.kind],
              full ? "px-[14px]" : "px-[11px] whitespace-pre",
            )}
          >
            {line.kind === "hunk" || line.kind === "meta"
              ? line.text
              : `${marker(line.kind)}${line.text}`}
          </div>
        ))}
      </div>
    </PreviewShell>
  );
}

export interface ArtifactDiffTabProps {
  /** `null` when there is no patch to draw — `note` then says why. */
  diff: ArtifactDiff | null;
  /** The refusal, the loading line, or the "only one version" sentence. */
  note?: string | null;
  size: PreviewSize;
  fromTime?: string | null;
  toTime?: string | null;
  className?: string;
}

export function ArtifactDiffTab({
  diff,
  note = null,
  size,
  fromTime,
  toTime,
  className,
}: ArtifactDiffTabProps) {
  if (diff === null) {
    return (
      <PreviewUnavailable size={size} note={note} className={className}>
        No earlier version to compare against.
      </PreviewUnavailable>
    );
  }
  return (
    <DiffView
      patch={diff.patch}
      size={size}
      fromLabel={`v${diff.from}`}
      toLabel={`v${diff.to}`}
      fromTime={fromTime}
      toTime={toTime}
      className={className}
    />
  );
}
