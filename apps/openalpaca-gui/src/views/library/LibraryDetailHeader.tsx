/**
 * The Library detail header (DESIGN_SPEC §3.31).
 *
 * Pinned above a scrolling body (§2.4) — unlike the Work detail, which scrolls
 * as one column. Presentational: every action is a callback, so the view keeps
 * the daemon calls and this keeps the layout.
 *
 * `Reveal` is deliberately labelled as the design labels it, but the only route
 * behind it (`POST /v1/files/{id}/open`) *opens* the file with the host's
 * default app rather than revealing it in Finder (API_MAP §2.3); the view says
 * so in its toast.
 */

import {
  Button,
  FileBadge,
  Tab,
  languageFromName,
  pinVariant,
  toFileKind,
} from "@/components/ui";
import type { Artifact } from "@/lib/api/unbacked";
import type { ArtifactTab } from "@/stores/ui";

import { relativeTime } from "./format";

const TABS: readonly { id: ArtifactTab; label: string }[] = [
  { id: "preview", label: "Preview" },
  { id: "diff", label: "Diff" },
  { id: "history", label: "History" },
];

export interface LibraryDetailHeaderProps {
  artifact: Artifact;
  pinned: boolean;
  tab: ArtifactTab;
  onTabChange: (tab: ArtifactTab) => void;
  onTogglePin: () => void;
  onExport: () => void;
  onReveal: () => void;
  /** Absent when the artifact is not attributed to a run. */
  onJumpRun?: () => void;
}

export function LibraryDetailHeader({
  artifact,
  pinned,
  tab,
  onTabChange,
  onTogglePin,
  onExport,
  onReveal,
  onJumpRun,
}: LibraryDetailHeaderProps) {
  const agent = artifact.agent_template_id ?? artifact.agent_id;
  return (
    <div>
      <div className="flex items-start gap-[12px]">
        <FileBadge
          kind={toFileKind(artifact.kind)}
          size={32}
          language={languageFromName(artifact.name)}
        />

        <div className="min-w-0 flex-1">
          <h2 className="m-0 text-2xl font-semibold tracking-tighter text-ink">
            {artifact.name}
          </h2>
          <div className="mt-[6px] flex flex-wrap items-center gap-[10px] font-mono text-xs text-muted-fg">
            <span>
              v{artifact.version} of {artifact.version_count}
            </span>
            {agent !== null && <span>{agent}</span>}
            {artifact.task_title !== null && onJumpRun !== undefined && (
              <button
                type="button"
                onClick={onJumpRun}
                className="cursor-pointer border-none bg-transparent p-0 font-mono text-xs text-blue underline hover:text-blue-hover"
              >
                {artifact.task_title}
              </button>
            )}
            <span>{relativeTime(artifact.updated_at)}</span>
          </div>
        </div>

        <div className="flex shrink-0 gap-[6px]">
          <Button
            variant={pinVariant(pinned)}
            aria-pressed={pinned}
            onClick={onTogglePin}
            className="rounded-md px-[10px] py-[5px] text-sm-plus"
          >
            {pinned ? "★ Pinned" : "☆ Pin"}
          </Button>
          <Button variant="ghostSm" onClick={onExport}>
            Export
          </Button>
          <Button variant="ghostSm" onClick={onReveal}>
            Reveal
          </Button>
        </div>
      </div>

      <div
        role="tablist"
        aria-label="Artifact views"
        className="mt-[16px] flex gap-[2px] border-b border-line-subtle"
      >
        {TABS.map((entry) => (
          <Tab
            key={entry.id}
            size="library"
            label={entry.label}
            active={tab === entry.id}
            onClick={() => onTabChange(entry.id)}
          />
        ))}
      </div>
    </div>
  );
}
