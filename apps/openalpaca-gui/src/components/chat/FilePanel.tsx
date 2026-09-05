/**
 * `FilePanel` (DESIGN_SPEC §3.23) — mode (b) of the chat aside.
 *
 * The aside is **one slot with two modes**: this panel and the Work pane are
 * never both mounted (§8.4). `‹ Work` restores the Work pane; `›` collapses the
 * aside entirely.
 *
 * What the daemon can and cannot serve here:
 *   * the artifact switcher lists the Library — GAP-04, there is no artifact
 *     listing route, so the dropdown shows its head and says so rather than
 *     inventing rows;
 *   * `Diff` and `History` are GAP-05 — nothing versioned exists in storage;
 *   * `Preview` renders whatever the caller can actually resolve (a real file's
 *     extracted text) through `preview` — `FilePanelSlot` fills that slot with
 *     the shared §3.25 renderers at `size="compact"`.
 */

import { FileBadge, Tab, type FileKind } from "@/components/ui";
import { cn } from "@/lib/cn";
import type { ArtifactTab } from "@/stores/ui";

export interface PanelArtifact {
  id: string;
  name: string;
  kind: FileKind;
  language?: string | null;
  /** Only when a real version is known (GAP-05 keeps this `null` today). */
  version?: number | null;
  agent?: string | null;
  runId?: string | null;
}

export interface PickerItem {
  id: string;
  name: string;
  kind: FileKind;
  language?: string | null;
  pinned?: boolean;
  stamp?: string | null;
}

export interface FilePanelProps {
  /** `null` when the id cannot be resolved to anything real. */
  artifact: PanelArtifact | null;
  /** Names the missing API when `artifact` is `null`. */
  artifactNote: string | null;

  tab: ArtifactTab;
  onTabChange: (tab: ArtifactTab) => void;

  pickerOpen: boolean;
  onTogglePicker: () => void;
  onClosePicker: () => void;
  pickerItems: readonly PickerItem[];
  /** Shown inside the dropdown when the Library cannot be listed. */
  pickerNote: string | null;
  onPickArtifact: (artifactId: string) => void;

  onBackToWork: () => void;
  onClose: () => void;
  onOpenInLibrary: () => void;
  onJumpRun?: () => void;

  pinned: boolean;
  onTogglePin: () => void;

  /** Tab bodies. Each falls back to its gap note when not supplied. */
  preview?: React.ReactNode;
  previewNote?: string | null;
  diffNote?: string | null;
  historyNote?: string | null;
}

const TABS: readonly ArtifactTab[] = ["preview", "diff", "history"];
const TAB_LABEL: Record<ArtifactTab, string> = {
  preview: "Preview",
  diff: "Diff",
  history: "History",
};

function GapBody({ note }: { note: string }) {
  return (
    <div className="rounded-2xl border border-dashed border-line bg-raised px-[14px] py-[13px]">
      <p className="m-0 text-md text-muted-fg">Nothing to show here yet.</p>
      <p className="mt-[6px] mb-0 font-mono text-2xs-plus text-faint">{note}</p>
    </div>
  );
}

export function FilePanel({
  artifact,
  artifactNote,
  tab,
  onTabChange,
  pickerOpen,
  onTogglePicker,
  onClosePicker,
  pickerItems,
  pickerNote,
  onPickArtifact,
  onBackToWork,
  onClose,
  onOpenInLibrary,
  onJumpRun,
  pinned,
  onTogglePin,
  preview,
  previewNote = null,
  diffNote = null,
  historyNote = null,
}: FilePanelProps) {
  const name = artifact?.name ?? "Unknown file";
  const kind: FileKind = artifact?.kind ?? "term";

  return (
    <>
      <div className="relative shrink-0 border-b border-line-subtle bg-canvas">
        <div className="flex items-center gap-[7px] px-[12px] pt-[9px] pb-[7px]">
          <button
            type="button"
            onClick={onBackToWork}
            className="flex shrink-0 cursor-pointer items-center gap-[4px] rounded-md border border-line bg-transparent px-[9px] py-[5px] font-sans text-sm-plus leading-[normal] font-medium text-secondary hover:bg-rail focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
          >
            ‹ Work
          </button>

          <button
            type="button"
            aria-haspopup="listbox"
            aria-expanded={pickerOpen}
            onClick={onTogglePicker}
            className="flex min-w-0 flex-1 cursor-pointer items-center gap-[8px] rounded-lg border border-line bg-raised px-[10px] py-[6px] text-left hover:border-line-hover focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
          >
            <FileBadge
              kind={kind}
              size={17}
              language={artifact?.language ?? null}
            />
            <span className="min-w-0 flex-1 truncate text-base-plus font-medium">
              {name}
            </span>
            <span aria-hidden className="text-2xs text-muted-fg">
              {pickerOpen ? "▴" : "▾"}
            </span>
          </button>

          <button
            type="button"
            aria-label="Close file panel"
            onClick={onClose}
            className="shrink-0 cursor-pointer border-none bg-transparent px-[6px] py-[2px] text-xl leading-[normal] text-muted-fg hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
          >
            ›
          </button>
        </div>

        <div
          role="tablist"
          aria-label="Artifact view"
          className="flex items-center px-[12px]"
        >
          {TABS.map((value) => (
            <Tab
              key={value}
              label={TAB_LABEL[value]}
              size="panel"
              active={tab === value}
              onClick={() => onTabChange(value)}
            />
          ))}
          <button
            type="button"
            onClick={onOpenInLibrary}
            className="ml-auto cursor-pointer border-none bg-transparent px-0 py-[4px] font-mono text-2xs-plus text-muted-fg hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
          >
            Library ↗
          </button>
        </div>

        {pickerOpen && (
          <>
            <div
              role="presentation"
              onClick={onClosePicker}
              className="fixed inset-0 z-30"
            />
            <div
              role="listbox"
              aria-label="Library files"
              className="sc absolute top-[calc(100%-34px)] right-[12px] left-[12px] z-[31] max-h-[340px] overflow-y-auto rounded-2xl border border-line-popover bg-raised shadow-popover"
            >
              <p className="m-0 border-b border-line-hair-2 px-[11px] pt-[8px] pb-[6px] font-mono text-[8.5px] tracking-eyebrow-w text-faint uppercase">
                Library
                {pickerItems.length > 0 && ` · ${pickerItems.length} files`}
              </p>
              {pickerItems.map((item) => (
                <button
                  key={item.id}
                  type="button"
                  role="option"
                  aria-selected={item.id === artifact?.id}
                  onClick={() => onPickArtifact(item.id)}
                  className={cn(
                    "flex w-full cursor-pointer items-center gap-[9px] border-0 border-b border-b-line-hair-3 px-[11px] py-[8px] text-left",
                    item.id === artifact?.id ? "bg-muted-2" : "bg-transparent",
                  )}
                >
                  <FileBadge
                    kind={item.kind}
                    size={16}
                    language={item.language ?? null}
                  />
                  <span className="min-w-0 flex-1 truncate text-base">
                    {item.name}
                  </span>
                  {item.pinned === true && (
                    <span aria-hidden className="text-2xs-plus text-gold">
                      ★
                    </span>
                  )}
                  {item.stamp != null && (
                    <span className="font-mono text-2xs text-faint">
                      {item.stamp}
                    </span>
                  )}
                </button>
              ))}
              {pickerNote !== null && (
                <p className="m-0 px-[11px] py-[10px] font-mono text-2xs-plus text-faint">
                  {pickerNote}
                </p>
              )}
            </div>
          </>
        )}
      </div>

      <div className="sc min-h-0 flex-1 overflow-y-auto px-[14px] pt-[13px] pb-[18px]">
        <div className="mb-[11px] flex flex-wrap items-center gap-[8px]">
          <span className="font-mono text-2xs-plus text-muted-fg">
            {[
              artifact?.version != null ? `v${artifact.version}` : null,
              artifact?.agent ?? null,
            ]
              .filter((part) => part !== null)
              .join(" · ")}
          </span>
          {artifact?.runId != null && onJumpRun !== undefined && (
            <button
              type="button"
              onClick={onJumpRun}
              className="cursor-pointer border-none bg-transparent p-0 font-mono text-2xs-plus text-blue underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
            >
              {artifact.runId}
            </button>
          )}
          <button
            type="button"
            aria-pressed={pinned}
            onClick={onTogglePin}
            className={cn(
              "ml-auto cursor-pointer rounded-[5px] px-[8px] py-[2px] text-xs-plus leading-[normal] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
              pinned
                ? "border border-gold-line bg-gold-tint text-gold-ink"
                : "border border-line bg-transparent text-secondary",
            )}
          >
            {pinned ? "★ Pinned" : "☆ Pin"}
          </button>
        </div>

        {artifact === null && artifactNote !== null ? (
          <GapBody note={artifactNote} />
        ) : tab === "preview" ? (
          (preview ?? (
            <GapBody
              note={previewNote ?? "Preview not available for this file"}
            />
          ))
        ) : tab === "diff" ? (
          <GapBody note={diffNote ?? "Artifact diff not yet available"} />
        ) : (
          <GapBody
            note={historyNote ?? "Artifact version history not yet available"}
          />
        )}
      </div>
    </>
  );
}
