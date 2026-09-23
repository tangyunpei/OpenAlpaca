/**
 * Whether this install can answer at all, as the empty transcript reads it
 * (D-G).
 *
 * "Setup completed" is **derived**, never stored: it is `GET /v1/status`'s
 * `llm.effective_default_model !== null`. There is no flag, no migration and
 * no new status field — and there must not be one, because this is a liveness
 * fact. Stop Ollama, or let a cloud key expire, and a finished setup becomes an
 * unfinished one. A stored "done" bit would then be a lie that survives a
 * restart.
 *
 * That is also why the surface is an inline card and never a blocking modal:
 * the value is re-read on every 30 s poll, and a modal driven by it would
 * re-fire in the middle of someone's work the first time their laptop slept
 * and Ollama did not come back — blocking the window over a condition that
 * often heals by itself. A card in an empty transcript that vanishes on the
 * next poll costs nothing when it is wrong.
 *
 * Three states, and the middle one is the whole point: a daemon that has not
 * answered yet is *unknown*, and unknown renders nothing. `GET /v1/status` has
 * no `initialData` and a first boot answers slowly (the embedding model is
 * downloaded before anything else), so reading `undefined` as "no model" would
 * put "you have no model" on screen at every cold start of a perfectly
 * configured install. Hence `=== undefined` and `=== null` below, and never a
 * falsy test.
 */

import type { DaemonLlmStatus } from "@/lib/api/types";
import { effectiveModelNote } from "@/lib/model-availability";

export type FirstRunState = "unknown" | "no-model" | "ready";

export function firstRunState(
  effective: string | null | undefined,
): FirstRunState {
  if (effective === undefined) return "unknown";
  return effective === null ? "no-model" : "ready";
}

/** The card's heading — fixed, and deliberately not a question. */
export const FIRST_RUN_TITLE = "No model is connected yet";

/**
 * The body. `effectiveModelNote(llm, "elsewhere")` is the daemon's own reading
 * of its own config, so it is preferred whenever it has something to say; the
 * fallback covers the one case it returns `null` for a `no-model` install,
 * which is a daemon serving `default_model_routable: true` and
 * `effective_default_model: null` — a contradiction, but not one worth
 * rendering blank.
 */
export function firstRunBody(llm: DaemonLlmStatus | null | undefined): string {
  return (
    effectiveModelNote(llm, "elsewhere") ??
    "No chat model is available. Open Settings → Models & keys and turn a provider on; a local one needs no key."
  );
}

/** The second line, which never changes: what the two routes to a model are. */
export const FIRST_RUN_HINT =
  "A local model needs no API key — turn Ollama on and the daemon asks it what you have installed. A cloud provider needs its switch on first, then a key.";

/** The one button. */
export const FIRST_RUN_ACTION = "Open Models & keys";
