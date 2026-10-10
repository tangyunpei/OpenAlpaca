/**
 * `LibraryRow` (DESIGN_SPEC §3.30).
 *
 * Presentational, and typed against the `Artifact` resource
 * `GET /v1/artifacts` serves — never a fixture.
 *
 * The subtitle is `agent · run · when`. Any of the three can legitimately be
 * absent on the wire (`agent_id` and `task_title` are nullable in the proposal),
 * so missing parts are dropped rather than filled with a placeholder.
 *
 * A file an agent wrote into a project (storage design §13, ST-12) is named by
 * its path in the project — its stored name — with a `project` tag, and its
 * subtitle starts with the project's folder name: the Library lists every
 * project's results, and two projects' `README.md` must not read the same.
 *
 * The name is the agent's, so it is drawn through `lib/hidden-chars`: each
 * hidden character as a `<U+XXXX>` escape, with the count on a line of its
 * own under the subtitle. A name with no hidden character draws as it is.
 */

import {
  EscapedText,
  FileBadge,
  Tag,
  languageFromName,
  toFileKind,
} from "@/components/ui";
import type { Artifact } from "@/lib/api/artifacts";
import { cn } from "@/lib/cn";
import { escapeHiddenLine, hiddenNotice } from "@/lib/hidden-chars";

import { projectName, relativeTime } from "./format";

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
    artifact.in_project ? projectName(artifact.project_root) : null,
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
  const name = escapeHiddenLine(artifact.name);
  const notice = hiddenNotice(name.hidden);
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
          <span
            className={cn(
              "text-base-plus font-medium text-ink",
              // A name holding an escape wraps: an ellipsis could cut off one
              // the count says is there.
              name.hidden > 0 ? "wrap-anywhere" : "truncate",
            )}
          >
            <EscapedText escaped={name} />
          </span>
          {artifact.in_project && <Tag value="project" />}
          {pinned && (
            <span aria-label="Pinned" role="img" className="text-sm text-gold">
              ★
            </span>
          )}
        </span>
        <span className="mt-[3px] block truncate font-mono text-2xs-plus text-muted-fg">
          {artifactSubtitle(artifact)}
        </span>
        {notice !== null && (
          <span className="mt-[2px] block text-2xs-plus text-red-ink">
            {notice}
          </span>
        )}
      </span>
    </button>
  );
}
