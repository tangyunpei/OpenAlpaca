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
 *
 * A file an agent wrote into a project (storage design §13) is the project's
 * own file, with no versions (R37): the meta row says `project` and the
 * project's folder name (its root on the tooltip) where `vN of M` would be,
 * the tab strip holds Preview alone, and `Open` is drawn only when the daemon
 * says it opens that kind (`openable`, ST-18) — a `.sh` or a `Makefile` gets
 * `Reveal in Finder` alone.
 *
 * The name and the summary are paths an agent chose, so both are drawn
 * through `lib/hidden-chars`: each hidden character as a `<U+XXXX>` escape
 * (the summary's tooltip too), and one line under them with the two counts
 * summed. Text with no hidden character draws as it is.
 */

import {
  Button,
  EscapedText,
  FileBadge,
  Tab,
  Tag,
  languageFromName,
  pinVariant,
  toFileKind,
} from "@/components/ui";
import type { Artifact } from "@/lib/api/artifacts";
import { cn } from "@/lib/cn";
import { escapeHiddenLine, hiddenNotice } from "@/lib/hidden-chars";
import { canReveal, revealLabel } from "@/lib/reveal";
import type { ArtifactTab } from "@/stores/ui";

import { projectName, relativeTime } from "./format";

const TABS: readonly { id: ArtifactTab; label: string }[] = [
  { id: "preview", label: "Preview" },
  { id: "diff", label: "Diff" },
  { id: "history", label: "History" },
];

/** A project file's one view: it has no versions to diff or list (R37). */
const PROJECT_FILE_TABS = TABS.filter((entry) => entry.id === "preview");

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
  const tabs = artifact.in_project ? PROJECT_FILE_TABS : TABS;
  const project = artifact.in_project
    ? projectName(artifact.project_root)
    : null;
  const name = escapeHiddenLine(artifact.name);
  const summary =
    artifact.summary !== null && artifact.summary.trim() !== ""
      ? escapeHiddenLine(artifact.summary)
      : null;
  const notice = hiddenNotice(name.hidden + (summary?.hidden ?? 0));
  return (
    <div>
      <div className="flex items-start gap-[12px]">
        <FileBadge
          kind={toFileKind(artifact.kind)}
          size={32}
          language={languageFromName(artifact.name)}
        />

        <div className="min-w-0 flex-1">
          <h2
            className={cn(
              "m-0 text-2xl font-semibold tracking-tighter text-ink",
              name.hidden > 0 && "wrap-anywhere",
            )}
          >
            <EscapedText escaped={name} />
          </h2>
          {summary !== null && (
            <p
              title={summary.text}
              className={cn(
                "m-0 mt-[4px] text-base text-muted-fg",
                // An ellipsis could cut off an escape the count says is there.
                summary.hidden > 0 ? "wrap-anywhere" : "truncate",
              )}
            >
              <EscapedText escaped={summary} />
            </p>
          )}
          {notice !== null && (
            <p className="m-0 mt-[4px] text-2xs-plus text-red-ink">{notice}</p>
          )}
          <div className="mt-[6px] flex flex-wrap items-center gap-[10px] font-mono text-xs text-muted-fg">
            {artifact.in_project ? (
              <>
                <Tag value="project" />
                {project !== null && (
                  <span title={artifact.project_root ?? undefined}>
                    {project}
                  </span>
                )}
              </>
            ) : (
              <span>
                v{artifact.version} of {artifact.version_count}
              </span>
            )}
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
          {artifact.openable && (
            <Button variant="ghostSm" onClick={onOpen}>
              Open
            </Button>
          )}
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
        {tabs.map((entry) => (
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
