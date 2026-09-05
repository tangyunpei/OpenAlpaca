/**
 * Formatting the Settings sections need.
 *
 * `gapDetail` is shared with the Library and lives beside the gap registry, in
 * `lib/unavailable.ts`.
 */

/** The design's `29 Aug` conversation stamp. */
export function shortDate(iso: string | null | undefined): string {
  if (iso === null || iso === undefined) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "—";
  return `${date.getDate()} ${date.toLocaleString("en-GB", { month: "short" })}`;
}

/**
 * `3 Sep 14:22:41` — when a row entered its current state (ADR-030 §8's
 * `since`). Boot counts as a transition, so this is never older than the
 * daemon's start; the stamp says so by carrying the day as well as the clock.
 */
export function whenStamp(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "—";
  return `${shortDate(iso)} ${timeOfDay(iso)}`;
}

/** `14:22:41` — the design's time-of-day stamp for log rows. */
export function timeOfDay(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "—";
  return date.toLocaleTimeString("en-GB", { hour12: false });
}

/** The design's `41k tokens`. */
export function compactCount(value: number): string {
  if (value < 1000) return `${value}`;
  if (value < 1_000_000) return `${Math.round(value / 100) / 10}k`;
  return `${Math.round(value / 100_000) / 10}M`;
}

export function percent(fraction: number): string {
  return `${Math.round(fraction * 100)}%`;
}
