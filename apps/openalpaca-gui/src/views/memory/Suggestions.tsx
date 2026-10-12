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
 *
 * **Hidden characters** (`lib/hidden-chars`). The row is where the owner
 * decides, and every text on it came from a file or from the model — the two
 * titles, both texts, the reason and the values of what else it changes —
 * so each hidden character in them is a highlighted `<U+XXXX>`, never drawn
 * raw (the titles, the one-line reason and each changed value take the
 * one-line escape, the texts the arguments' one), and one line under the
 * texts counts them in the words every approval surface uses.
 *
 * **What else changes** (the L8 close-out's A4). A suggestion that changes
 * the note's key, aliases, tags or validity says so, one row per field —
 * the note's values, then the suggestion's — or a change of metadata alone
 * would read as two identical texts. Alpaca's own suggestion carries no
 * "Why": its text may copy the note's, whose reason is that note's.
 * Display only: Replace and Keep mine answer the notes by id and revision,
 * as read.
 *
 * The daemon serves the replaced note's text cut at 600 bytes and the
 * suggestion's at 4 KiB, and Replace retires the whole note: a text served
 * cut ends with how many characters the cut left out (`cutNotice`, the chat
 * apps' words for a cut prompt), and the rest is in the note's detail.
 */

import { Button, EscapedText, Eyebrow } from "@/components/ui";
import type { MemorySuggestion } from "@/lib/api/memories";
import {
  cutNotice,
  escapeHidden,
  escapeHiddenLine,
  hiddenNotice,
} from "@/lib/hidden-chars";

import {
  changeLabel,
  changeValues,
  savedByAlpaca,
  suggestionWhence,
  targetOriginLabel,
  targetTextLabel,
} from "./copy";
import { suggestionHidden } from "./suggestion-hidden";

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
  const target = escapeHiddenLine(item.target.title);
  const title = escapeHiddenLine(item.title);
  const excerpt = escapeHidden(item.target.excerpt);
  const knowledge = escapeHidden(item.knowledge);
  const reason = item.reason === null ? null : escapeHiddenLine(item.reason);
  const changes = item.changes ?? [];
  const notice = hiddenNotice(suggestionHidden({ ...item, changes }));
  return (
    <li className="min-w-0 text-base leading-[1.5] wrap-anywhere text-amber-ink">
      {/* Design 09 §3.5: “Replace “X” (saved by you) with “B”?” */}
      <button
        type="button"
        onClick={() => onOpen(item.id)}
        className="block max-w-full cursor-pointer border-none bg-transparent p-0 text-left font-medium text-ink hover:underline"
      >
        Replace “<EscapedText escaped={target} />” (
        {targetOriginLabel(item.target.origin)}) with “
        <EscapedText escaped={title} />
        ”?
      </button>
      <dl className="m-0 mt-[4px] grid grid-cols-[auto_1fr] gap-x-[8px] gap-y-[2px]">
        <dt className="font-mono text-2xs-plus text-muted-fg">
          {targetTextLabel(item.target.origin)}
        </dt>
        <dd className="m-0 min-w-0 break-words text-secondary">
          <EscapedText escaped={excerpt} />
          <CutNote leftOut={item.target.excerpt_left_out} />
        </dd>
        <dt className="font-mono text-2xs-plus text-muted-fg">New</dt>
        <dd className="m-0 min-w-0 break-words text-ink">
          <EscapedText escaped={knowledge} />
          <CutNote leftOut={item.knowledge_left_out} />
        </dd>
        {changes.map((change) => (
          <ChangeRow key={change.field} change={change} />
        ))}
        {reason !== null && (
          <>
            <dt className="font-mono text-2xs-plus text-muted-fg">Why</dt>
            <dd className="m-0 min-w-0 break-words text-secondary">
              <EscapedText escaped={reason} />
            </dd>
          </>
        )}
      </dl>
      {notice !== null && (
        <p className="mt-[3px] mb-0 text-red-ink">{notice}</p>
      )}
      <span className="mt-[3px] block font-mono text-2xs-plus text-muted-fg">
        {suggestionWhence(
          item.origin,
          when,
          savedByAlpaca(item.origin, item.observed_at),
        )}
        {item.older_count > 0 &&
          ` · ${item.older_count} older suggestion${item.older_count === 1 ? "" : "s"} for this note`}
      </span>
      <div className="mt-[6px] flex gap-[6px]">
        <Button
          variant="primarySm"
          aria-label={`Replace ${target.text}`}
          disabled={busy}
          onClick={() => onReplace(item)}
        >
          Replace
        </Button>
        <Button
          variant="ghostSm"
          aria-label={`Keep mine: ${target.text}`}
          disabled={busy}
          onClick={() => onKeep(item)}
        >
          Keep mine
        </Button>
      </div>
    </li>
  );
}

/**
 * One metadata field the suggestion changes: its label, the note's values,
 * then the suggestion's — each value through the one-line escape, as the
 * titles are (the L8 close-out's A4). A change of metadata alone would
 * otherwise show two identical texts.
 */
function ChangeRow({
  change,
}: {
  change: MemorySuggestion["changes"][number];
}) {
  const from = escapeHiddenLine(changeValues(change.from));
  const to = escapeHiddenLine(changeValues(change.to));
  return (
    <>
      <dt className="font-mono text-2xs-plus text-muted-fg">
        {changeLabel(change.field)}
      </dt>
      <dd className="m-0 min-w-0 break-words text-ink">
        <EscapedText escaped={from} /> → <EscapedText escaped={to} />
      </dd>
    </>
  );
}

/** How much a served text's cut left out, after the text; nothing when whole. */
function CutNote({ leftOut }: { leftOut: number }) {
  const note = cutNotice(leftOut);
  return note === null ? null : <span className="text-muted-fg"> {note}</span>;
}
