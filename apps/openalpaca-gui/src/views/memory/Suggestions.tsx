/**
 * *Suggestions* (Learning v2, design 09 §3.1, §3.5; owner decisions MD-11
 * and G2).
 *
 * When the owner says something that changes a note they saved or edited by
 * hand, learning does not overwrite it: it writes a **suggestion** — a
 * candidate that names the note it would replace and is left out of answers
 * until the owner decides. Each row asks the question in full, with **both
 * texts**, because a suggestion can merely restate the note it names (the
 * learner may never have been shown it), and the owner can only tell by
 * reading the two side by side.
 *
 *   Replace   confirms the suggestion, sending its revision as `If-Match` and
 *             the replaced note's as `target_revision` — both as this window
 *             read them, so an edit of either since is a `409`, never a
 *             silent retirement. The old note is kept as history.
 *   Keep mine dismisses it: the undo route, a withdrawal that forgets nothing
 *             else the owner said.
 *
 * The section hides when nothing is waiting.
 */

import { Button, Eyebrow } from "@/components/ui";
import type { MemorySuggestion } from "@/lib/api/memories";

import { suggestionQuestion, suggestionWhence } from "./copy";

export interface SuggestionsProps {
  items: readonly MemorySuggestion[];
  /** The suggestion a Replace or Keep mine is in flight for. */
  pendingId: string | null;
  onReplace: (suggestion: MemorySuggestion) => void;
  onKeep: (suggestion: MemorySuggestion) => void;
  onOpen: (id: string) => void;
}

export function Suggestions({
  items,
  pendingId,
  onReplace,
  onKeep,
  onOpen,
}: SuggestionsProps) {
  if (items.length === 0) return null;
  return (
    <section
      aria-label="Suggestions"
      className="mb-[10px] rounded-xl border border-amber-line bg-amber-surface px-[10px] py-[9px]"
    >
      <Eyebrow className="mb-[6px]">Suggestions</Eyebrow>
      <ul className="m-0 flex list-none flex-col gap-[12px] p-0">
        {items.map((item) => (
          <SuggestionRow
            key={item.id}
            item={item}
            busy={pendingId === item.id}
            onReplace={onReplace}
            onKeep={onKeep}
            onOpen={onOpen}
          />
        ))}
      </ul>
    </section>
  );
}

function SuggestionRow({
  item,
  busy,
  onReplace,
  onKeep,
  onOpen,
}: {
  item: MemorySuggestion;
  busy: boolean;
  onReplace: (suggestion: MemorySuggestion) => void;
  onKeep: (suggestion: MemorySuggestion) => void;
  onOpen: (id: string) => void;
}) {
  const when = item.observed_at ?? item.created_at;
  return (
    <li className="min-w-0 text-base leading-[1.5] text-amber-ink">
      <button
        type="button"
        onClick={() => onOpen(item.id)}
        className="block max-w-full cursor-pointer border-none bg-transparent p-0 text-left font-medium text-ink hover:underline"
      >
        {suggestionQuestion(item.target.title, item.target.origin, item.title)}
      </button>
      <dl className="m-0 mt-[4px] grid grid-cols-[auto_1fr] gap-x-[8px] gap-y-[2px]">
        <dt className="font-mono text-2xs-plus text-muted-fg">Yours</dt>
        <dd className="m-0 min-w-0 break-words text-secondary">
          {item.target.excerpt}
        </dd>
        <dt className="font-mono text-2xs-plus text-muted-fg">New</dt>
        <dd className="m-0 min-w-0 break-words text-ink">{item.knowledge}</dd>
        {item.reason !== null && (
          <>
            <dt className="font-mono text-2xs-plus text-muted-fg">Why</dt>
            <dd className="m-0 min-w-0 break-words text-secondary">
              {item.reason}
            </dd>
          </>
        )}
      </dl>
      <span className="mt-[3px] block font-mono text-2xs-plus text-muted-fg">
        {suggestionWhence(item.origin, when)}
        {item.older_count > 0 &&
          ` · ${item.older_count} older suggestion${item.older_count === 1 ? "" : "s"} for this note`}
      </span>
      <div className="mt-[6px] flex gap-[6px]">
        <Button
          variant="primarySm"
          aria-label={`Replace ${item.target.title}`}
          disabled={busy}
          onClick={() => onReplace(item)}
        >
          Replace
        </Button>
        <Button
          variant="ghostSm"
          aria-label={`Keep mine: ${item.target.title}`}
          disabled={busy}
          onClick={() => onKeep(item)}
        >
          Keep mine
        </Button>
      </div>
    </li>
  );
}
