/**
 * Formatting the Library rows need.
 *
 * `gapDetail` used to be duplicated here and in `views/settings/format.ts`; it
 * now lives beside the registry it reads, in `lib/unavailable.ts`.
 */

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** The design's `2m ago` subline stamp. Future timestamps read as `just now`. */
export function relativeTime(iso: string, now: Date = new Date()): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const delta = now.getTime() - then;
  if (delta < MINUTE) return "just now";
  if (delta < HOUR) return `${Math.floor(delta / MINUTE)}m ago`;
  if (delta < DAY) return `${Math.floor(delta / HOUR)}h ago`;
  return `${Math.floor(delta / DAY)}d ago`;
}
