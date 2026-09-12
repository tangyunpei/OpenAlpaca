/**
 * `HistoryTab` / `VersionRow` (DESIGN_SPEC §3.25), in both sizes.
 *
 * `compact` is the chat aside's file panel and `full` the Library detail, the
 * same pair every other artifact renderer takes — one component so the two
 * cannot drift.
 *
 * Versions are newest first and index 0 takes the raised treatment. The
 * `+3 −1` pair is the daemon's own stored count for that write, computed from
 * the same `similar` diff the Diff tab renders — so the number on a row and the
 * patch a reader then opens agree. It is `null` on v1 (nothing to change from)
 * and for the kinds that are not text, and is simply omitted there.
 *
 * A `null` author is not "unknown" — it is §4.8's **hand edit**: the daemon
 * hashes an artifact's head on every read and, when the bytes are not the ones
 * its row describes, records them as a version nobody in OpenAlpaca wrote.
 * Editing a produced file by hand is the point of putting artifacts in the
 * project, so the row says so instead of leaving the author line blank.
 */

import type { PreviewSize } from "@/components/work/preview";
import { cn } from "@/lib/cn";
import type { ArtifactVersion } from "@/lib/api/artifacts";

import { relativeTime } from "./format";

/** The author line for a version with no `author_agent_id` (§4.8). */
export const EDITED_BY_HAND = "edited by hand";

export interface HistoryTabProps {
  versions: readonly ArtifactVersion[];
  size?: PreviewSize;
}

export function HistoryTab({ versions, size = "full" }: HistoryTabProps) {
  const ordered = [...versions].sort((a, b) => b.version - a.version);
  return (
    <div
      className={cn(
        "flex flex-col",
        size === "full" ? "max-w-[660px] gap-[8px]" : "gap-[6px]",
      )}
    >
      {ordered.map((version, index) => (
        <VersionRow
          key={version.version}
          version={version}
          latest={index === 0}
          size={size}
        />
      ))}
    </div>
  );
}

interface VersionRowProps {
  version: ArtifactVersion;
  latest: boolean;
  size: PreviewSize;
}

function VersionRow({ version, latest, size }: VersionRowProps) {
  const full = size === "full";
  return (
    <div
      className={cn(
        "flex items-start gap-[12px] rounded-2xl border",
        full ? "px-[14px] py-[12px]" : "px-[11px] py-[9px]",
        latest
          ? "border-line-popover bg-raised"
          : "border-line-subtle bg-inactive",
      )}
    >
      <span className="w-[26px] shrink-0 font-mono text-sm font-medium text-ink">
        v{version.version}
      </span>
      <span className="min-w-0 flex-1">
        <span
          className={cn(
            "block leading-[1.5] text-ink",
            full ? "text-base-plus" : "text-base",
          )}
        >
          {version.note}
        </span>
        <span className="mt-[3px] block font-mono text-2xs-plus text-muted-fg">
          {version.author_agent_id ?? EDITED_BY_HAND}
        </span>
      </span>
      {version.added_lines !== null && version.removed_lines !== null && (
        <span className="flex shrink-0 items-center gap-[6px] font-mono text-xs">
          <span className="text-green">{`+${version.added_lines}`}</span>
          <span className="text-red">{`−${version.removed_lines}`}</span>
        </span>
      )}
      <span className="shrink-0 font-mono text-xs text-muted-fg">
        {relativeTime(version.created_at)}
      </span>
    </div>
  );
}
