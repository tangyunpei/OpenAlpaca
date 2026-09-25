/**
 * "Notes used in this answer" — the memory redesign's `done.memory_used`
 * (design §5 step 8, §7 *Surfaces*; plan 3.9).
 *
 * The notes the turn delivered to the model: recalled automatically for the
 * question (`recall`), pinned (`pinned`), listed for it in the recall's index
 * block without their text (`index`, design 09 §13.2), or fetched by the
 * model itself with `memory_search`/`memory_read` (`tool`). Each opens in the
 * Memory view, where a wrong one can be corrected or forgotten — the point of
 * saying which notes shaped an answer is that the owner can do something
 * about one.
 *
 * The muted-note shape of `SkippedAttachmentsNote` (§3.15), and **live only**
 * for the same reason: nothing persists `memory_used`, so a reloaded
 * transcript has nothing to rebuild the note from (`GET /v1/memories/
 * recently-used` is the after-the-fact read).
 */

export interface MemoryUsedEntry {
  id: string;
  /** The note's title, or `null` while unknown — or when it is gone since. */
  title: string | null;
  reason: string;
}

/** Why the note reached the model, in the reader's words. */
export function usedReasonLabel(reason: string): string {
  switch (reason) {
    case "recall":
      return "recalled";
    case "pinned":
      return "pinned";
    case "index":
      return "listed";
    case "tool":
      return "looked up";
    default:
      return reason;
  }
}

export interface MemoryUsedNoteProps {
  items: readonly MemoryUsedEntry[];
  onOpen: (id: string) => void;
}

export function MemoryUsedNote({ items, onOpen }: MemoryUsedNoteProps) {
  if (items.length === 0) return null;
  return (
    <section
      aria-label="Notes used in this answer"
      className="mt-[10px] rounded-xl border border-line-subtle bg-muted px-[13px] py-[11px]"
    >
      <p className="mt-0 mb-[5px] font-mono text-2xs-plus tracking-eyebrow text-tertiary uppercase">
        Notes used in this answer
      </p>
      <ul className="m-0 list-none p-0">
        {/* One id can come twice — listed by the recall, then opened with
            memory_read — so the key is the row, not the note. */}
        {items.map((entry, index) => (
          <li
            key={`${entry.id}:${entry.reason}:${index}`}
            className="text-base-plus text-secondary"
          >
            <button
              type="button"
              onClick={() => onOpen(entry.id)}
              className="cursor-pointer border-none bg-transparent p-0 text-base-plus text-ink underline decoration-line-strong underline-offset-2 hover:decoration-ink"
            >
              {entry.title ?? `note ${entry.id.slice(0, 8)}`}
            </button>{" "}
            — {usedReasonLabel(entry.reason)}
          </li>
        ))}
      </ul>
    </section>
  );
}
