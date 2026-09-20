/**
 * Formatting the Library rows need.
 *
 * `gapDetail` used to be duplicated here and in `views/settings/format.ts`; it
 * now lives beside the registry it reads, in `lib/unavailable.ts`.
 */

import { timestampMs } from "@/lib/time";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/**
 * The design's `2m ago` subline stamp. Future timestamps read as `just now`.
 *
 * `parseTimestamp`, not `Date.parse`: a file row's `created_at` is the
 * daemon's zone-less UTC, and read as local time every file west of Greenwich
 * was written "just now" for the next several hours (G3).
 */
export function relativeTime(iso: string, now: Date = new Date()): string {
  const then = timestampMs(iso);
  if (then === null) return "";
  const delta = now.getTime() - then;
  if (delta < MINUTE) return "just now";
  if (delta < HOUR) return `${Math.floor(delta / MINUTE)}m ago`;
  if (delta < DAY) return `${Math.floor(delta / HOUR)}h ago`;
  return `${Math.floor(delta / DAY)}d ago`;
}
