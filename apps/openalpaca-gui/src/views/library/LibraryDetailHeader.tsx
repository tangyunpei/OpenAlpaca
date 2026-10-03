/**
 * The Library detail header (DESIGN_SPEC §3.31).
 *
 * Pinned above a scrolling body (§2.4) — unlike the Work detail, which scrolls
 * as one column. Presentational: every action is a callback, so the view keeps
 * the daemon calls and this keeps the layout.
 *
 * `Open` opens the file in its default app, through the daemon
 * (`POST /v1/files/{id}/open`, API_MAP §2.3) — on the daemon's host. `Reveal in
 * Finder` ("Show in folder" off macOS) shows the file in its folder through the
 * Tauri shell's `reveal_path` command, so it is drawn only inside the app's own
 * window (`canReveal`); the Vite preview has no shell to ask.
 *
 * The artifact's `summary`, when it has one, is a muted line under the name:
 * for a `file_write` artifact it is the path the file was written as
 * (`notes/tea.md`, `Makefile`), which the stored name may no longer show.
 */

import {
  Button,
  FileBadge,
  Tab,
  languageFromName,
  pinVariant,
  toFileKind,
} from "@/components/ui";
import type { Artifact } from "@/lib/api/artifacts";
import { canReveal, revealLabel } from "@/lib/reveal";
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
  /** Opens the file in its default app. */
  onOpen: () => void;
  /** Shows the file in its folder; drawn only when `canReveal()`. */
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
  onOpen,
  onReveal,
  onJumpRun,
}: LibraryDetailHeaderProps) {
  const agent = artifact.agent_template_id ?? artifact.agent_id;
  const summary =
    artifact.summary !== null && artifact.summary.trim() !== ""
      ? artifact.summary
      : null;
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
          {summary !== null && (
            <p
              title={summary}
              className="m-0 mt-[4px] truncate text-base text-muted-fg"
            >
              {summary}
            </p>
          )}
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
          <Button variant="ghostSm" onClick={onOpen}>
            Open
          </Button>
          {canReveal() && (
            <Button variant="ghostSm" onClick={onReveal}>
              {revealLabel()}
            </Button>
          )}
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
