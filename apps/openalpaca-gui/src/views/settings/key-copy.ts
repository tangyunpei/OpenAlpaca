/**
 * What the `Add key` form says, and whether it may save (D-F).
 *
 * Pure, for the reason `models-copy.ts` and `provider-toggle.ts` are: what
 * this screen *claims* is the part that can be wrong, and a claim is only
 * worth making if a test can read it back.
 *
 * The one rule this file exists for: **a key is never saved against a
 * switched-off provider.** `PUT /v1/settings/llm` would accept it — encrypt
 * it, write it to `llm.toml`, answer `200` — and change nothing a user can
 * observe, because the router drops a disabled provider's models from its
 * catalogue at boot and only the enable (`set_provider_enabled`) puts them
 * back. So the form refuses before any typing, says why, and offers the
 * switch. It never flips the bit itself, and it never saves first and offers
 * afterwards.
 */

import { ApiError } from "@/lib/http";
import type {
  KeyPriorityValue,
  KeySourceValue,
  KeyValidationResult,
} from "@/lib/api/types";

export type KeySavePlan =
  /** Ready: an enabled, keyed provider with a secret typed. */
  | { action: "save" }
  /** The provider needs no key at all (L1) — there is nothing to save. */
  | { action: "keyless"; reason: string }
  /** The provider is switched off. Saving would be a no-op (D-F). */
  | { action: "refuse-disabled"; reason: string }
  /** Nothing typed yet. */
  | { action: "incomplete"; reason: string };

export const INCOMPLETE_NOTE = "Paste the key to save it.";

/**
 * The refusal: the *consequence* first ("would do nothing"), then the
 * mechanism, then the instruction. It never says "invalid" — the key is not —
 * and never "error", because nothing failed.
 */
export function disabledRefusal(provider: string): string {
  return (
    `${provider} is switched off, so a key saved now would do nothing. ` +
    `The daemon drops a disabled provider's models from its catalogue, and only turning the ` +
    `provider on puts them back — the key would be encrypted, written to llm.toml, and ignored. ` +
    `Turn ${provider} on first, then save the key.`
  );
}

/** The whole page for a provider that needs no key (Ollama). */
export function keylessNote(provider: string): string {
  return (
    `${provider} needs no API key. Turning it on is the whole setup: the daemon then asks your ` +
    `running ${provider} what is installed and registers every chat model it reports.`
  );
}

/**
 * Evaluated in this order, and the order is the design: a keyless provider
 * has nothing to refuse, and **disabled is checked before the secret**, so the
 * user is told the real blocker rather than sent to fetch a key that would be
 * ignored.
 */
export function keySavePlan(input: {
  provider: string;
  enabled: boolean;
  requiresKey: boolean;
  secret: string;
}): KeySavePlan {
  if (!input.requiresKey) {
    return { action: "keyless", reason: keylessNote(input.provider) };
  }
  if (!input.enabled) {
    return {
      action: "refuse-disabled",
      reason: disabledRefusal(input.provider),
    };
  }
  if (input.secret.trim().length === 0) {
    return { action: "incomplete", reason: INCOMPLETE_NOTE };
  }
  return { action: "save" };
}

/**
 * The CLI's own id shape (`llm_keys.rs`, `keys_add`): `<provider>_<unix
 * seconds>`. The field is `id` — `key_id` is silently dropped by serde and the
 * daemon mints a `key_<uuid8>` instead, so a key added with the wrong field
 * name comes back under an id nobody asked for.
 */
export function mintKeyId(provider: string, now: Date = new Date()): string {
  return `${provider}_${Math.floor(now.getTime() / 1000)}`;
}

export const KEY_PRIORITIES: readonly KeyPriorityValue[] = [
  "primary",
  "fallback",
];

/**
 * The sources the form offers — the five `openalpaca llm keys add` asks for,
 * sent as the GUI's own snake_case `KeySourceValue`s. (The CLI sends the
 * display labels instead; a row must therefore never trust `KeyInfo.source`
 * to be one of these.)
 */
export const KEY_SOURCE_OPTIONS: readonly {
  value: KeySourceValue;
  label: string;
}[] = [
  { value: "api_console", label: "API Console" },
  { value: "claude_code", label: "Claude Code" },
  { value: "codex", label: "Codex" },
  { value: "environment", label: "Environment" },
  { value: "other", label: "Other" },
];

/** A save the daemon refused, in the daemon's two real refusals' own terms. */
export function saveErrorCopy(error: Error): string {
  if (error instanceof ApiError) {
    if (error.status === 400 && error.code === "INVALID_KEY_FORMAT") {
      return "The daemon would not take that key: it is empty.";
    }
    if (error.status === 500 && error.code === "ENCRYPTION_FAILED") {
      return `The key could not be encrypted, so nothing was written. ${error.message}`;
    }
    if (error.status === 500 && error.code === "DISK_WRITE_FAILED") {
      return `llm.toml could not be written, so nothing was saved. ${error.message}`;
    }
  }
  return `Could not save the key — ${error.message}`;
}

/**
 * What `POST /v1/settings/llm/validate` found. The reason lives in
 * **`format_error`**; there is no `message` field and there never was.
 */
export function validationLine(result: KeyValidationResult): string {
  if (!result.valid) {
    return `Key rejected — ${result.format_error ?? "Key validation failed"}`;
  }
  let line = "Key looks valid.";
  if (result.models_available.length > 0) {
    line += ` · ${result.models_available.length} ${
      result.models_available.length === 1 ? "model" : "models"
    } visible`;
  }
  if (result.tier !== null) line += ` · tier ${result.tier}`;
  return line;
}

/**
 * A check that could not be made. The route answers `504
 * KEY_VALIDATION_TIMEOUT` for *every* failure of the check, so this says
 * "could not check", never "timed out" — and its second sentence is the point:
 * a failed check is not a reason to refuse a save.
 */
export function validationFailureLine(error: Error): string {
  return (
    `Could not check the key — the daemon did not answer (${error.message}). ` +
    "You can save it anyway; the router will find out on the first call."
  );
}
