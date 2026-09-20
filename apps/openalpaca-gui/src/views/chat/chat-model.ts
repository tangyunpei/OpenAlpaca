/**
 * Which model the composer holds, and what it may send (G9).
 *
 * The composer's label is a promise: every send carries the held id as
 * `model`, and a request that *names* a model the router cannot serve is
 * refused outright (`400 UNKNOWN_MODEL` — the L3 fallback ladder is for turns
 * that name nothing). So the held id must never be one the daemon cannot
 * route, and the store seeds from the daemon's **effective** default — `GET
 * /v1/status`'s `llm.effective_default_model`, what a request naming nothing
 * would really reach — not from the *configured* one. On a local-only install
 * those two differ: `[orchestrator] model` is whatever the file says (a Claude
 * id, out of the box) and nothing serves it.
 *
 * Kept pure, and separate from the view, because the interesting half is the
 * cases: a held model whose provider was switched off, a tag that vanished
 * from Ollama, a daemon that can route nothing at all, and a models list that
 * has not answered yet — in which last case the right move is to do nothing,
 * never to clear a model on a fetch that merely failed.
 */

import type { ModelEntry } from "@/lib/api/types";

export interface ChatModelInputs {
  /** What the store holds — `null` before the first seed. */
  held: string | null;
  /**
   * `GET /v1/status` → `llm.effective_default_model`.
   *
   * `undefined` is *unknown* — the status query has not answered, or this
   * daemon serves no `llm` block at all — and nothing is decided on it.
   * `null` is the daemon's own answer that it can route nothing.
   */
  effective: string | null | undefined;
  /** `GET /v1/models`, once it answered; `undefined` while loading or failed. */
  models: readonly ModelEntry[] | undefined;
}

export type ChatModelDecision =
  | { action: "keep" }
  /** Nothing was held: take the daemon's effective default, silently. */
  | { action: "seed"; model: string }
  /** The held id is gone from the catalogue; `model: null` = nothing to put there. */
  | { action: "replace"; model: string | null; previous: string };

export function decideChatModel({
  held,
  effective,
  models,
}: ChatModelInputs): ChatModelDecision {
  if (held === null) {
    return typeof effective === "string" && effective !== ""
      ? { action: "seed", model: effective }
      : { action: "keep" };
  }
  // A list nobody has read is not a list the model is missing from.
  if (models === undefined) return { action: "keep" };
  if (models.some((entry) => entry.id === held)) return { action: "keep" };
  if (effective === undefined) return { action: "keep" };
  // The daemon still names it; replacing it with itself would only loop.
  if (effective === held) return { action: "keep" };
  return { action: "replace", model: effective, previous: held };
}

/**
 * What the replacement says out loud — the pick toast's shape, plus the half
 * that explains itself: the id the window was holding is gone.
 */
export function modelReplacedToast(
  next: string | null,
  provider: string | null,
  previous: string,
): string {
  return next === null
    ? `${previous} is no longer available — no model is routable`
    : `Chat model → ${next} (${provider ?? "unknown provider"}) — ${previous} is no longer available`;
}
