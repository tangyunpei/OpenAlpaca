/**
 * `WrittenArtifactCard` — a file an agent just wrote, shown inline in the
 * transcript.
 *
 * The same house shape as `RunReportCard` (§3.12): a status strip over a body,
 * with the eyebrow saying what happened and the right-hand mono segment
 * carrying the identity. Deliberately smaller than an `ArtifactCard` (§3.13):
 * an `artifact_written` frame carries the name, kind and version and *no
 * content*, so this card links to the file rather than previewing lines it does
 * not have. Everything it shows came off the frame.
 *
 * Beside `Open in Library`: `Open` (the file in its default app, through the
 * daemon) and `Reveal in Finder` (the file in its folder, through the Tauri
 * shell — drawn only inside the app's window, `canReveal`).
 *
 * A file written into the project (storage design §13, ST-12) is the project's
 * own file with a pointer row, not an artifact: the eyebrow says `in the
 * project`, the name is its path there, no version is drawn (it has none,
 * R37), and `Open` is drawn only when the frame's `openable` says the daemon
 * opens that kind (ST-18) — a `.sh` or a `Makefile` gets Reveal alone.
 *
 * The name is a path an agent chose, so it is drawn through
 * `lib/hidden-chars`: each hidden character as a `<U+XXXX>` escape, with the
 * count on a line of its own under the name. A name with no hidden character
 * draws as it is.
 */

import { Button, EscapedText, FileBadge, type FileKind } from "@/components/ui";
import { cn } from "@/lib/cn";
import { escapeHiddenLine, hiddenNotice } from "@/lib/hidden-chars";
import { canReveal, revealLabel } from "@/lib/reveal";

export interface WrittenArtifactCardProps {
  name: string;
  kind: FileKind;
  language?: string | null;
  version: number;
  /** A result file in the project: named there, with no version drawn. */
  inProject: boolean;
  /** ST-18: whether `Open` is drawn — the frame's `openable`. */
  openable: boolean;
  /** `13:41`, or `null` when the frame's stamp is unreadable. */
  time: string | null;
  /** Opens the file in its default app. */
  onOpen: () => void;
  /** Shows the file in its folder; drawn only when `canReveal()`. */
  onReveal: () => void;
  /** Opens the file in the Library. */
  onOpenInLibrary: () => void;
}

export function WrittenArtifactCard({
  name,
  kind,
  language = null,
  version,
  inProject,
  openable,
  time,
  onOpen,
  onReveal,
  onOpenInLibrary,
}: WrittenArtifactCardProps) {
  const shown = escapeHiddenLine(name);
  const notice = hiddenNotice(shown.hidden);
  return (
    <section className="mb-[26px] overflow-hidden rounded-3xl border border-line bg-raised">
      <header className="flex items-center gap-[9px] border-b border-line-hair bg-sunken px-[14px] py-[10px]">
        <span
          aria-hidden
          className="block h-[6px] w-[6px] shrink-0 rounded-full bg-blue"
        />
        <span className="font-mono text-2xs-plus tracking-eyebrow text-blue uppercase">
          File written
          {inProject && " · in the project"}
          {time !== null && ` · ${time}`}
        </span>
        {!inProject && (
          <span className="ml-auto font-mono text-xs text-faint">
            v{version}
          </span>
        )}
      </header>

      <div className="flex items-center gap-[10px] px-[14px] py-[12px]">
        <FileBadge kind={kind} size={19} language={language} />
        <span
          className={cn(
            "min-w-0 flex-1 text-md font-medium",
            // A name holding an escape wraps: an ellipsis could cut off one
            // the count says is there.
            shown.hidden > 0 ? "wrap-anywhere" : "truncate",
          )}
        >
          <EscapedText escaped={shown} />
        </span>
        {openable && (
          <Button variant="ghostXs" onClick={onOpen} className="shrink-0">
            Open
          </Button>
        )}
        {canReveal() && (
          <Button variant="ghostXs" onClick={onReveal} className="shrink-0">
            {revealLabel()}
          </Button>
        )}
        <button
          type="button"
          onClick={onOpenInLibrary}
          className="shrink-0 cursor-pointer rounded-md border border-line bg-transparent px-[9px] py-[4px] font-sans text-sm-plus leading-[normal] font-medium text-secondary hover:bg-muted-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
        >
          Open in Library
        </button>
      </div>
      {notice !== null && (
        <p className="m-0 px-[14px] pb-[12px] text-2xs-plus text-red-ink">
          {notice}
        </p>
      )}
    </section>
  );
}
