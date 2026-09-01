/**
 * `ArtifactCard` (DESIGN_SPEC §3.13) — an artifact shown inline in the
 * transcript.
 *
 * Three of the card's parts have no backing API and are handled honestly
 * rather than faked:
 *   * the version chip (`v2`) — GAP-05, nothing versioned exists in storage,
 *     so the chip is rendered only when a version is actually known;
 *   * `Diff v1→v2` — GAP-05 again; the control stays visible (it is part of the
 *     design) and reports the gap when pressed instead of drawing a diff;
 *   * the preview body — the only real source is the file's extracted text
 *     (`FileAsset.extracted_text`). With no lines, the body says so and names
 *     the missing API; it never invents plausible-looking content.
 *
 * Pins are deliberately client-side (GAP-12 — no pinned column exists, and a
 * per-machine pin is the correct model for one anyway).
 */

import { Button, FileBadge, type FileKind } from "@/components/ui";
import { cn } from "@/lib/cn";

export interface ArtifactCardProps {
  name: string;
  kind: FileKind;
  language?: string | null;
  /** Rendered only when a real version number is known (GAP-05). */
  version?: number | null;
  /** The first lines of the artifact; the first is styled as its heading. */
  previewLines?: readonly string[];
  /** `… 34 more lines` — omitted when the preview is complete. */
  remainingLines?: number | null;
  /** `connector audit · review_agent` — whichever halves are known. */
  context?: string | null;
  /** Shown in place of a preview when there is nothing real to show. */
  unavailableNote?: string | null;
  pinned?: boolean;
  onOpen?: () => void;
  onTogglePin?: () => void;
  onDiff?: () => void;
  className?: string;
}

export function ArtifactCard({
  name,
  kind,
  language = null,
  version = null,
  previewLines = [],
  remainingLines = null,
  context = null,
  unavailableNote = null,
  pinned = false,
  onOpen,
  onTogglePin,
  onDiff,
  className,
}: ArtifactCardProps) {
  return (
    <section
      className={cn(
        "overflow-hidden rounded-3xl border border-line bg-raised shadow-card",
        className,
      )}
    >
      <header className="flex items-center gap-[10px] border-b border-line-card px-[14px] py-[11px]">
        <FileBadge kind={kind} size={19} language={language} />
        <span className="flex-1 truncate text-md font-medium">{name}</span>
        {version !== null && (
          <span className="rounded-sm bg-muted px-[6px] py-[2px] font-mono text-xs text-tertiary">
            v{version}
          </span>
        )}
        <Button variant="primarySm" onClick={onOpen}>
          Open
        </Button>
      </header>

      <div className="px-[14px] py-[13px] font-mono text-sm-plus leading-[1.75] text-preview">
        {previewLines.length > 0 ? (
          <>
            {previewLines.map((line, index) => (
              <div
                key={index}
                className={cn(
                  "truncate",
                  index === 0 && "font-medium text-ink",
                )}
              >
                {line}
              </div>
            ))}
            {remainingLines !== null && remainingLines > 0 && (
              <div className="text-faint">… {remainingLines} more lines</div>
            )}
          </>
        ) : (
          <div className="text-faint">
            {unavailableNote ?? "No preview available for this file."}
          </div>
        )}
      </div>

      <footer className="flex items-center gap-[8px] border-t border-line-card bg-sunken px-[14px] py-[9px]">
        {context !== null && context !== "" && (
          <span className="truncate font-mono text-xs text-muted-fg">
            {context}
          </span>
        )}
        <div className="ml-auto flex shrink-0 gap-[6px]">
          {onDiff !== undefined && (
            <Button variant="ghostXs" onClick={onDiff}>
              Diff
            </Button>
          )}
          {onTogglePin !== undefined && (
            <Button
              variant="ghostXs"
              aria-pressed={pinned}
              onClick={onTogglePin}
            >
              {pinned ? "★ Pinned" : "☆ Pin"}
            </Button>
          )}
        </div>
      </footer>
    </section>
  );
}
