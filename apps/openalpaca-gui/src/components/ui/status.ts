/**
 * Run-status vocabulary shared by `StatusDot`, `StatusLabel` and the rail
 * (DESIGN_SPEC §3.20, §4.3).
 *
 * The design models five states. The daemon's `TaskStatus` has six: it also
 * emits `failed`, and `completed` is the wire spelling of `done`
 * (`lib/api/types.ts#toRunStatus`). Painting a failed run as `DONE` would state
 * something untrue about the run, so `failed` is carried as a sixth status
 * styled from the palette's error tokens — the single documented addition to
 * §3.20's table.
 */

import type { RunStatus, TaskStatusValue } from "@/lib/api/types";

export type UiStatus =
  "running" | "queued" | "paused" | "done" | "cancelled" | "failed";

/** `completed` → `done`; every other value is already a `UiStatus`. */
export function toUiStatus(status: TaskStatusValue | RunStatus): UiStatus {
  return status === "completed" ? "done" : status;
}

/** The uppercase word `StatusLabel` renders. */
export const STATUS_TEXT: Record<UiStatus, string> = {
  running: "RUNNING",
  queued: "QUEUED",
  paused: "PAUSED",
  done: "DONE",
  cancelled: "CANCELLED",
  failed: "FAILED",
};

/** Sentence-case, for `aria-label` on the dot. */
export function statusAria(status: UiStatus): string {
  return STATUS_TEXT[status].toLowerCase();
}

/** Only `running` pulses (§1.7). */
export function statusPulses(status: UiStatus): boolean {
  return status === "running";
}

/** `railRuns` — everything that is not finished (§4.2). */
export function isLive(status: UiStatus): boolean {
  return status !== "done" && status !== "cancelled" && status !== "failed";
}

/** `activeCount` — drives the Work nav badge and the "N running" pill (§4.2). */
export function isActive(status: UiStatus): boolean {
  return status === "running" || status === "queued" || status === "paused";
}
