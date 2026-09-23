/**
 * What a stored key's row says (Settings → Models & keys).
 *
 * Pure, for the same reason `key-copy.ts` is. The one trap here is
 * `KeyInfo.source`: its TypeScript type is the snake_case `KeySourceValue`
 * union, but the daemon stores whatever string it was sent and echoes it back
 * unchanged — and `openalpaca llm keys add` sends its display labels
 * (`"API Console"`, `"Claude Code"`, …). So a key the CLI wrote carries a
 * value the union does not contain, and nothing in TypeScript will say so.
 * `formatKeySource` therefore takes a plain `string`, maps the six known
 * values, and passes anything else through **verbatim** — never a blank,
 * never a crash.
 */

import type { KeyInfo, KeySourceValue } from "@/lib/api/types";

const SOURCE_LABELS: Record<KeySourceValue, string> = {
  api_console: "API Console",
  claude_code: "Claude Code",
  claude_max_pro: "Claude Max/Pro",
  codex: "Codex",
  environment: "Environment",
  other: "Other",
};

function isKnownSource(source: string): source is KeySourceValue {
  return Object.hasOwn(SOURCE_LABELS, source);
}

/** A key's source as a person reads it; an unknown string is shown as-is. */
export function formatKeySource(source: string | null | undefined): string {
  if (source === null || source === undefined || source.trim() === "") {
    return "no source recorded";
  }
  return isKnownSource(source) ? SOURCE_LABELS[source] : source;
}

/**
 * The key's health as `GET /v1/settings/llm` reports it. `unknown` is said as
 * unknown — a key nobody has used yet has no health, and the row does not
 * invent one.
 */
export function formatKeyHealth(status: string): string {
  switch (status) {
    case "healthy":
      return "healthy";
    case "rate_limited":
      return "rate limited";
    case "error":
      return "error";
    case "unknown":
      return "health unknown";
    default:
      return status;
  }
}

/** One line per key: the daemon's own masking, priority, source and health. */
export function keyRowSummary(key: KeyInfo): string {
  return [
    key.masked_secret,
    key.priority,
    formatKeySource(key.source),
    formatKeyHealth(key.status),
  ].join(" · ");
}
