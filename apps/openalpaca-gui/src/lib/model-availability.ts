/**
 * What the window says about whether this install can answer at all.
 *
 * Moved out of `views/settings/models-copy.ts` so the chat first-run card and
 * the Settings banner read the *same* four branches off the same block: two
 * screens that disagreed about whether this install can answer would be worse
 * than either of them being wrong. It lives in `lib/` rather than being
 * imported across views because the sentence is not a Settings fact — it is
 * the daemon's own reading of its `llm` block (`GET /v1/status`).
 */

import type { DaemonLlmStatus } from "@/lib/api/types";

/**
 * Where the sentence is drawn — which decides only the call to action. On the
 * Settings screen the switch really is below the sentence; anywhere else it is
 * a screen away and has to be named. Nothing else about the four branches
 * differs.
 */
export type NoteAudience = "settings-body" | "elsewhere";

function turnOnSentence(where: NoteAudience): string {
  return where === "settings-body"
    ? "Turn a provider on below; a local one needs no key."
    : "Open Settings → Models & keys and turn a provider on; a local one needs no key.";
}

/**
 * What to say when the configured chat model is not the one that would answer
 * (L3) — `null` when there is nothing to say.
 *
 * `[orchestrator] model` is allowed to name a model this install cannot serve:
 * every shipped agent template pins a Claude id, and on a machine with only a
 * local provider the ladder answers with something else. A picker that shows
 * the configured id alone is then a picker that disagrees with every reply.
 *
 * It is also allowed to name **nothing**: an install that has never set one
 * sends `default_model: null` (M8), which is not the same sentence — there is
 * no configured id to report as unavailable, only whatever the ladder found.
 */
export function effectiveModelNote(
  llm: DaemonLlmStatus | null | undefined,
  where: NoteAudience = "settings-body",
): string | null {
  // A daemon that does not say — no router, or one built before the block —
  // is not second-guessed.
  if (llm === null || llm === undefined) return null;
  if (llm.default_model_routable) return null;
  if (llm.default_model === null || llm.default_model === "") {
    return llm.effective_default_model === null
      ? `No chat model is configured, and none is available. ${turnOnSentence(where)}`
      : `No chat model is configured — using ${llm.effective_default_model}`;
  }
  if (llm.effective_default_model !== null) {
    return `configured: ${llm.default_model} — not available, using ${llm.effective_default_model}`;
  }
  return `configured: ${llm.default_model} — not available, and no model is. ${turnOnSentence(where)}`;
}
