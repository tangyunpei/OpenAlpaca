/**
 * `LibraryRow` (DESIGN_SPEC §3.30).
 *
 * Presentational, and typed against the *proposed* `Artifact` resource
 * (API_MAP §3, GAP-04) rather than any fixture — the day `GET /v1/artifacts`
 * lands, this row renders it unchanged.
 *
 * The subtitle is `agent · run · when`. Any of the three can legitimately be
 * absent on the wire (`agent_id` and `task_title` are nullable in the proposal),
 * so missing parts are dropped rather than filled with a placeholder.
 */

import { FileBadge, languageFromName, toFileKind } from "@/components/ui";
import type { Artifact } from "@/lib/api/unbacked";
import { cn } from "@/lib/cn";

import { relativeTime } from "./format";

export interface LibraryRowProps {
  artifact: Artifact;
  active: boolean;
  pinned: boolean;
  onSelect: (artifactId: string) => void;
}

export function artifactSubtitle(
  artifact: Artifact,
  now: Date = new Date(),
): string {
  const parts = [
    artifact.agent_template_id ?? artifact.agent_id,
    artifact.task_title,
    relativeTime(artifact.updated_at, now),
  ];
  return parts
    .filter((part): part is string => part !== null && part !== "")
    .join(" · ");
}

export function LibraryRow({
  artifact,
  active,
  pinned,
  onSelect,
}: LibraryRowProps) {
  const kind = toFileKind(artifact.kind);
  return (
    <button
      type="button"
      aria-current={active ? "true" : undefined}
      onClick={() => onSelect(artifact.id)}
      className={cn(
        "flex w-full cursor-pointer items-center gap-[10px] rounded-xl px-[10px] py-[9px] text-left",
        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
        active
          ? "border border-line-popover bg-raised"
          : "border border-transparent bg-transparent hover:bg-muted-2",
      )}
    >
      <FileBadge
        kind={kind}
        size={17}
        language={languageFromName(artifact.name)}
      />
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-[6px]">
          <span className="truncate text-base-plus font-medium text-ink">
            {artifact.name}
          </span>
          {pinned && (
            <span aria-label="Pinned" role="img" className="text-sm text-gold">
              ★
            </span>
          )}
        </span>
        <span className="mt-[3px] block truncate font-mono text-2xs-plus text-muted-fg">
          {artifactSubtitle(artifact)}
        </span>
      </span>
    </button>
  );
}
