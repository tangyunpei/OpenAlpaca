/**
 * `HistoryTab` / `VersionRow`, full size (DESIGN_SPEC §3.25).
 *
 * Versions are newest first and index 0 takes the raised treatment. The
 * `+3 −1` pair is the daemon's own stored count for that write, computed from
 * the same `similar` diff the Diff tab renders — so the number on a row and the
 * patch a reader then opens agree. It is `null` on v1 (nothing to change from)
 * and for the kinds that are not text, and is simply omitted there.
 */

import { cn } from "@/lib/cn";
import type { ArtifactVersion } from "@/lib/api/artifacts";

import { relativeTime } from "./format";

export interface HistoryTabProps {
  versions: readonly ArtifactVersion[];
}

export function HistoryTab({ versions }: HistoryTabProps) {
  const ordered = [...versions].sort((a, b) => b.version - a.version);
  return (
    <div className="flex max-w-[660px] flex-col gap-[8px]">
      {ordered.map((version, index) => (
        <VersionRow
          key={version.version}
          version={version}
          latest={index === 0}
        />
      ))}
    </div>
  );
}

interface VersionRowProps {
  version: ArtifactVersion;
  latest: boolean;
}

function VersionRow({ version, latest }: VersionRowProps) {
  return (
    <div
      className={cn(
        "flex items-start gap-[12px] rounded-2xl border px-[14px] py-[12px]",
        latest
          ? "border-line-popover bg-raised"
          : "border-line-subtle bg-inactive",
      )}
    >
      <span className="w-[26px] shrink-0 font-mono text-sm font-medium text-ink">
        v{version.version}
      </span>
      <span className="min-w-0 flex-1">
        <span className="block text-base-plus leading-[1.5] text-ink">
          {version.note}
        </span>
        {version.author_agent_id !== null && (
          <span className="mt-[3px] block font-mono text-2xs-plus text-muted-fg">
            {version.author_agent_id}
          </span>
        )}
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
