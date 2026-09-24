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
