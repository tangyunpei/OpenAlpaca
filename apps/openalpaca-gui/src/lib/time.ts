/**
 * Reading the daemon's timestamps (G3).
 *
 * The daemon speaks two spellings and only one of them carries a zone:
 *
 *   * `DateTime<Utc>` serialises RFC 3339 — `2026-09-18T17:05:12.123Z`;
 *   * SQLite's own `datetime('now')`, which every persisted row written through
 *     a `CURRENT_TIMESTAMP` default carries, is `2026-09-18 17:05:12` — **UTC,
 *     with nothing to say so**.
 *
 * `new Date("2026-09-18 17:05:12")` reads the second as *local* time, so a
 * transcript in Los Angeles printed a message sent at 10:05 as `17:05`, and the
 * same rows sorted seven hours into the future — which is why a freshly sent
 * message (a real `Z` stamp) rendered *above* the whole conversation instead of
 * at the end of it, and looked lost until the turn finished (G5).
 *
 * So there is one parser, and every clock, stamp and sort in the window goes
 * through it. A value that is not a date at all answers `null`: the callers
 * print an em dash rather than `Invalid Date`, and inventing "now" for a row
 * whose time is unreadable would be a guess in a transcript.
 */

/** Already zoned: a `Z`, or a `±HH:MM` / `±HHMM` offset. */
const ZONED = /(Z|[+-]\d{2}:?\d{2})$/;

/**
 * The instant a daemon timestamp names, or `null` when it names none.
 *
 * A zone-less value is read as UTC, which is what the daemon wrote.
 */
export function parseTimestamp(value: string | null | undefined): Date | null {
  if (value === null || value === undefined) return null;
  let text = value.trim();
  if (text === "") return null;
  if (!text.includes("T") && text.includes(" ")) text = text.replace(" ", "T");
  if (!ZONED.test(text)) text = `${text}Z`;
  const date = new Date(text);
  return Number.isNaN(date.getTime()) ? null : date;
}

/** Epoch milliseconds, or `null` — `parseTimestamp` for arithmetic. */
export function timestampMs(value: string | null | undefined): number | null {
  return parseTimestamp(value)?.getTime() ?? null;
}
