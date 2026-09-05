/**
 * `DiffTab` (DESIGN_SPEC §3.25 `DiffTab`) — the standalone v1 → v2 view, in
 * both sizes.
 *
 * The `+9 / −2` counters are **counted from the patch**, not read off the
 * response: a counter that disagrees with the lines under it is worse than no
 * counter, and the proposed endpoint's `added_lines` is a convenience field.
 *
 * `ArtifactDiffTab` is the container: version history and diffs do not exist
 * anywhere in storage (GAP-05), so today it renders the design's empty copy and
 * names the route that would fill it.
 */

import { useMemo } from "react";

import { cn } from "@/lib/cn";
import type { ArtifactDiff } from "@/lib/api/unbacked";
import { isAvailable, type Availability } from "@/lib/unavailable";

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
  diff: Availability<ArtifactDiff>;
  size: PreviewSize;
  fromTime?: string | null;
  toTime?: string | null;
  className?: string;
}

export function ArtifactDiffTab({
  diff,
  size,
  fromTime,
  toTime,
  className,
}: ArtifactDiffTabProps) {
  if (!isAvailable(diff)) {
    return (
      <PreviewUnavailable size={size} note={diff.reason} className={className}>
        No earlier version to compare against.
      </PreviewUnavailable>
    );
  }
  return (
    <DiffView
      patch={diff.data.patch}
      size={size}
      fromLabel={`v${diff.data.from}`}
      toLabel={`v${diff.data.to}`}
      fromTime={fromTime}
      toTime={toTime}
      className={className}
    />
  );
}
