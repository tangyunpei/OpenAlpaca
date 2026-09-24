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
 * Undo is a forget refused (`409 MEMORY_NOT_UNDOABLE`) once the note stopped
 * being `extracted_user` — the owner edited or confirmed it and so made it
 * theirs — and an edit through this app is exactly that, which is also why
 * such a note leaves this list.
 */

import { Eyebrow } from "@/components/ui";
import type { MemoryListItem } from "@/lib/api/memories";

import { LEARNED_FROM_YOU } from "./copy";

export interface RecentlyLearnedProps {
  items: readonly MemoryListItem[];
  /** The note an undo is in flight for. */
  pendingId: string | null;
  onUndo: (item: MemoryListItem) => void;
  onOpen: (id: string) => void;
}

export function RecentlyLearned({
  items,
  pendingId,
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
        {items.map((item) => (
          <li key={item.id} className="min-w-0">
            <button
              type="button"
              onClick={() => onOpen(item.id)}
              className="block max-w-full cursor-pointer truncate border-none bg-transparent p-0 text-left text-base font-medium text-ink hover:underline"
            >
              {item.title}
            </button>
            <span className="mt-[2px] block font-mono text-2xs-plus text-muted-fg">
              {LEARNED_FROM_YOU} ·{" "}
              <button
                type="button"
                aria-label={`Undo ${item.title}`}
                disabled={pendingId === item.id}
                onClick={() => onUndo(item)}
                className="cursor-pointer border-none bg-transparent p-0 font-mono text-2xs-plus text-blue underline hover:text-blue-hover disabled:cursor-default disabled:text-faint"
              >
                Undo
              </button>
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}
