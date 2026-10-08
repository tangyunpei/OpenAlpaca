/**
 * The Memory view's words (memory redesign, design §7 *Surfaces*).
 *
 * Three sentences are the design's own and are pinned verbatim by tests: the
 * empty state, the revision conflict and the recently-learned row. The rest
 * name daemon facts — an origin, a freshness, a derived status — in the
 * owner's terms, and fall back to the daemon's own word for anything this
 * build does not know rather than guessing.
 *
 * The conflict sentence is the **client's**: the daemon's `409` carries the
 * service's words (the note id and its current revision, 3.8 review finding
 * 4), and what a person holding a draft should do next is this side's to say.
 */

import type {
  EffectiveStatus,
  Freshness,
  MemorySource,
  NoteOrigin,
  NoteScope,
} from "@/lib/api/memories";

/** Design §7, verbatim. */
export const MEMORY_EMPTY_COPY =
  "Nothing remembered yet. Ask Alpaca to remember something, or add a note.";

/**
 * The list's count beside its title: `1 note`, `2 notes`, `50+ notes` when
 * more pages wait. One is singular only when it is all there is.
 */
export function noteCount(count: number, more: boolean): string {
  const noun = count === 1 && !more ? "note" : "notes";
  return `${count}${more ? "+" : ""} ${noun}`;
}

/** Design §7, verbatim — shown over a draft that is kept. */
export const MEMORY_CONFLICT_COPY =
  "This note changed since you opened it. Reload to see the current version — your text is kept.";

/**
 * Design §7: a recently-learned row reads "Learned from what you said · Undo".
 * This is the words before the button.
 */
export const LEARNED_FROM_YOU = "Learned from what you said";

/** A confirm, archive or forget sent against a revision that has moved. */
export const MEMORY_ACTION_CONFLICT =
  "This note changed since you opened it — look at the current version and try again.";

/** Why a candidate is not in answers yet (design §6, MD-2). */
export const CANDIDATE_NOTE =
  "Inferred from the conversation, not said by you. It is not used in answers until you confirm it.";

/** What every candidate note ends with. */
const NOT_USED_YET = "It is not used in answers until you confirm it.";

/**
 * Whether a note "learned from what you said" was written by Alpaca's own
 * memory tools in chat rather than by learning (owner decision LQ11): origin
 * `extracted_user` with no `observed_at`, which learning stamps on every
 * note it writes and a tool call cannot set (the daemon's own test, LQ14).
 */
export function savedByAlpaca(
  origin: NoteOrigin,
  observedAt: unknown,
): boolean {
  return origin === "extracted_user" && (observedAt ?? null) === null;
}

/**
 * A candidate's note follows its origin (design 09 §3.5): a suggestion from
 * the owner's own words says so and names the note it would replace; one
 * Alpaca saved in chat says it suggested it (owner decision LQ11); an
 * inferred one says it is not the owner's words. `replaces` is the replaced
 * note's title, `null` when the candidate names none (or it is unknown yet).
 */
export function candidateNote(
  origin: NoteOrigin,
  replaces: string | null,
  byAlpaca = false,
): string {
  if (origin === "extracted_user" && byAlpaca) {
    return replaces === null
      ? `Suggested by Alpaca. ${NOT_USED_YET}`
      : `Suggested by Alpaca — would replace “${replaces}”. ${NOT_USED_YET}`;
  }
  if (origin === "extracted_user") {
    return replaces === null
      ? `From what you said. ${NOT_USED_YET}`
      : `From what you said — would replace “${replaces}”. ${NOT_USED_YET}`;
  }
  return replaces === null
    ? CANDIDATE_NOTE
    : `Inferred from the conversation, not said by you — would replace “${replaces}”. ${NOT_USED_YET}`;
}

/** Whose note a suggestion would replace, as its question says it. */
export function targetOriginLabel(origin: NoteOrigin): string {
  switch (origin) {
    case "user":
      return "saved by you";
    case "external":
      return "edited by hand";
    case "imported":
      return "imported";
    case "extracted_user":
      return "learned";
    default:
      return origin;
  }
}

/**
 * The label beside the text a suggestion would replace: “Yours” only when
 * the owner wrote it — saved, edited by hand or imported — and “Current”
 * for a note learning wrote.
 */
export function targetTextLabel(origin: NoteOrigin): string {
  return origin === "user" || origin === "external" || origin === "imported"
    ? "Yours"
    : "Current";
}

/**
 * Where a suggestion came from, and when: “From what you said on
 * 2026-09-24”, or “Suggested by Alpaca on 2026-09-24” for one Alpaca's
 * memory tools saved in chat (owner decision LQ11). A suggestion whose own
 * text changed after learning wrote it says who wrote the words now, and
 * when it was suggested.
 */
export function suggestionWhence(
  origin: NoteOrigin,
  when: string,
  byAlpaca = false,
): string {
  const day = when.slice(0, 10);
  switch (origin) {
    case "extracted_user":
      return byAlpaca
        ? `Suggested by Alpaca on ${day}`
        : `From what you said on ${day}`;
    case "extracted_other":
      return `Inferred from the conversation on ${day}`;
    case "user":
      return `Edited by you · suggested on ${day}`;
    default:
      return `${originLabel(origin)} · suggested on ${day}`;
  }
}

/** How a suggestion's card names a metadata field it changes (the L8 close-out's A4). */
export function changeLabel(field: string): string {
  switch (field) {
    case "fact_key":
      return "Key";
    case "aliases":
      return "Aliases";
    case "tags":
      return "Tags";
    case "valid_from":
      return "Valid from";
    case "valid_to":
      return "Valid until";
    default:
      return field;
  }
}

/** One side of a change as its card shows it: the values, or “none”. */
export function changeValues(values: readonly string[]): string {
  return values.length === 0 ? "none" : values.join(", ");
}

/** The toast after Replace — the old note stays, as history (design 09 §3.3). */
export function replacedToast(target: string): string {
  return `Replaced — “${target}” is kept as history`;
}

/** The toast after Keep mine. */
export const KEPT_YOUR_NOTE = "Kept your note";

/** The toast after undoing a replacement: the note it replaced is back. */
export function undoneToast(replaced: string | null, title: string): string {
  return replaced === null
    ? `Undone — “${title}” is not remembered any more`
    : `Undone — “${replaced}” is back`;
}

/** The kept versions are the service's own writes, the last five (design §2). */
export const HISTORY_NOTE =
  "OpenAlpaca keeps the last five versions it wrote. An edit made outside the app is not kept here.";

/** The partition names the scope selector shows. */
export function scopeLabel(scope: NoteScope): string {
  return scope === "project" ? "This project" : "Global";
}

/**
 * Who wrote a note, in the owner's words. The authority order (design §4) is
 * the daemon's business; this only says where the text came from.
 */
export function originLabel(origin: NoteOrigin): string {
  switch (origin) {
    case "user":
      return "Saved by you";
    case "extracted_user":
      return LEARNED_FROM_YOU;
    case "extracted_other":
      return "Inferred from the conversation";
    case "imported":
      return "Imported";
    case "external":
      return "Edited outside OpenAlpaca";
    default:
      return origin;
  }
}

/** The tag a note's derived status earns, or `null` for a plain active one. */
export function statusTag(status: EffectiveStatus): string | null {
  return status === "active" ? null : status;
}

/**
 * What a non-`verified` freshness means for the text on screen. `null` when
 * there is nothing to say.
 */
export function freshnessNote(freshness: Freshness): string | null {
  switch (freshness) {
    case "verified":
      return null;
    case "stale":
      return "This file changed on disk since the last scan; what is shown is what is there now.";
    case "unverified":
      return "OpenAlpaca cannot vouch for this file: it was not written by the app, or its record is missing.";
    default:
      return freshness;
  }
}

/** A cited DB handle as a reader sees it — never a path. */
export function sourceLabel(source: MemorySource): string {
  if (source.type === "task") return `run ${source.id.split("-")[0] ?? ""}`;
  if (source.type === "message") return `message ${source.id}`;
  return `${source.type} ${source.id}`;
}

/** The first eight characters of a revision — enough to tell five apart. */
export function shortRevision(revision: string): string {
  return revision.slice(0, 8);
}

/**
 * Why `This project` is off (storage design §3): a project is a git
 * repository, or a folder chosen with `Use` in Settings → Connection.
 * `.openalpaca` marks nothing any more, and no copy names it.
 */
export const NO_PROJECT_TITLE =
  "Choose a project in Settings → Connection: a git repository, or any folder you Use there.";

/** Said under the scope chips when This project was chosen and is not available. */
export const NO_PROJECT_LINE =
  "This window has no project, or its path is inside none (no git repository and no folder chosen with Use holds it), so Global is shown.";
