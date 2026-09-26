/**
 * *Recently learned* (memory redesign, design §6 *Promotion and correction*;
 * MD-2).
 *
 * The notes the background learner took from what the owner said — active at
 * once, origin `extracted_user`, newest first over the daemon's default seven
 * days — each with a **one-step** undo. One step, not the Forget button's two:
 * the design offers undo precisely because the owner did not ask for the note,
 * and taking back something nobody asked for should cost one click.
 *
 * Undo is a **withdrawal** (design 09 §3.3), refused (`409
 * MEMORY_NOT_UNDOABLE`) once the note stopped being `extracted_user` — the
 * owner edited or confirmed it and so made it theirs — and an edit through
 * this app is exactly that, which is also why such a note leaves this list.
 * A withdrawal forgets nothing else the owner said, and a learned note that
 * **replaced** an older one says so — "Learned from what you said · replaced
 * “A” · Undo" — because its undo brings that note back.
 *
 * **Hidden characters** (`lib/hidden-chars`). Undo is the owner's decision
 * on a note learning wrote, so the row shows every hidden character in its
 * title and in the replaced note's as a highlighted `<U+XXXX>` (the one-line
 * escape) and, under them, one line counting them in the words every
 * approval surface uses. Display only: Undo answers the note by id and
 * revision.
 */

import { EscapedText, Eyebrow } from "@/components/ui";
import type { MemoryListItem } from "@/lib/api/memories";
import { cn } from "@/lib/cn";
import { escapeHiddenLine, hiddenNotice } from "@/lib/hidden-chars";

import { LEARNED_FROM_YOU } from "./copy";

export interface RecentlyLearnedProps {
  items: readonly MemoryListItem[];
  /** The note an undo is in flight for. */
  pendingId: string | null;
  /**
   * Titles of the notes the rows replaced, by id; a row whose replaced note
   * is not in it yet says plainly that it was learned.
   */
  replacedTitles?: ReadonlyMap<string, string>;
  /** `replaced` is the title of the note the undo brings back, if any. */
  onUndo: (item: MemoryListItem, replaced: string | null) => void;
  onOpen: (id: string) => void;
}

export function RecentlyLearned({
  items,
  pendingId,
  replacedTitles = new Map(),
  onUndo,
  onOpen,
}: RecentlyLearnedProps) {
  return (
    <section
      aria-label="Recently learned"
      className="mb-[10px] rounded-xl border border-line-subtle bg-muted px-[10px] py-[9px]"
    >
      <Eyebrow className="mb-[6px]">Recently learned</Eyebrow>
      <ul className="m-0 flex list-none flex-col gap-[7px] p-0">
        {items.map((item) => {
          const replacedId = item.supersedes[0];
          const replaced =
            replacedId === undefined
              ? null
              : (replacedTitles.get(replacedId) ?? null);
          const title = escapeHiddenLine(item.title);
          const was = replaced === null ? null : escapeHiddenLine(replaced);
          const notice = hiddenNotice(title.hidden + (was?.hidden ?? 0));
          return (
            <li key={item.id} className="min-w-0">
              <button
                type="button"
                onClick={() => onOpen(item.id)}
                className={cn(
                  "block max-w-full cursor-pointer border-none bg-transparent p-0 text-left text-base font-medium text-ink hover:underline",
                  // A title holding an escape wraps: an ellipsis could cut
                  // off one the count says is there.
                  title.hidden > 0 ? "wrap-anywhere" : "truncate",
                )}
              >
                <EscapedText escaped={title} />
              </button>
              <span className="mt-[2px] block font-mono text-2xs-plus text-muted-fg">
                {/* Design 09 §3.5: “Learned from what you said · replaced “A” · Undo” */}
                {LEARNED_FROM_YOU}
                {was !== null && (
                  <>
                    {" "}
                    · replaced “<EscapedText escaped={was} />”
                  </>
                )}{" "}
                ·{" "}
                <button
                  type="button"
                  aria-label={`Undo ${title.text}`}
                  disabled={pendingId === item.id}
                  onClick={() => onUndo(item, replaced)}
                  className="cursor-pointer border-none bg-transparent p-0 font-mono text-2xs-plus text-blue underline hover:text-blue-hover disabled:cursor-default disabled:text-faint"
                >
                  Undo
                </button>
              </span>
              {notice !== null && (
                <span className="mt-[2px] block text-2xs-plus text-red-ink">
                  {notice}
                </span>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
