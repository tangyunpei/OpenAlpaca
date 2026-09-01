/**
 * The run action catalogue (DESIGN_SPEC §3.19 action bar, §3.26 action group).
 *
 * Three of the design's seven verbs do not exist on the daemon. They are still
 * rendered — a hidden affordance cannot be reported — but as **disabled**
 * controls whose tooltip names the missing route (API_MAP §3):
 *
 *   `Start now` / `Re-run`  GAP-06 — `apply_task_action` accepts exactly
 *                           `cancel`, `pause`, `resume`.
 *   `Queue follow-up`       GAP-03 — the storage and the `followup_queued`
 *                           event exist; no HTTP route does.
 *
 * `Steer` is the awkward one. It stays **enabled** because the design's own
 * handler (`r.steer`, §4.4) does not send anything: it aims the chat composer
 * at the run, which is pure client state and works today. What is missing is
 * the *addressed* steer — the daemon's only channel is the chat text prefix
 * `/steer …`, which targets the lane's active workflow and takes no
 * `task_id` (GAP-02) — so the tooltip says exactly that, and the disabled-verb
 * footnote under the detail action group repeats it in visible text rather
 * than hover-only.
 *
 * `Cancel` / `Pause` / `Resume` are real and wired to `POST /v1/tasks/{id}/action`.
 */

import type { UiStatus } from "@/components/ui";
import { GAPS, gapNote, type GapId } from "@/lib/unavailable";

export type RunActionId =
  | "pause"
  | "resume"
  | "start"
  | "cancel"
  | "steer"
  | "queue"
  | "jump"
  | "rerun";

export type RunActionTone = "secondary" | "danger";

export interface RunActionDescriptor {
  id: RunActionId;
  label: string;
  tone: RunActionTone;
  /** `false` ⇒ render the control disabled with `title` as its tooltip. */
  enabled: boolean;
  /** Tooltip. Set whenever a gap blocks or constrains the action. */
  title?: string;
  gap?: GapId;
}

/** "<gap note> · proposed <endpoint>" — the tooltip of every gapped control. */
export function gapTooltip(id: GapId): string {
  const gap = GAPS[id];
  return `${gapNote(gap)} · proposed ${gap.proposedEndpoint}`;
}

const blocked = (
  id: RunActionId,
  label: string,
  gap: GapId,
): RunActionDescriptor => ({
  id,
  label,
  tone: "secondary",
  enabled: false,
  title: gapTooltip(gap),
  gap,
});

const STEER: RunActionDescriptor = {
  id: "steer",
  label: "Steer",
  tone: "secondary",
  // Enabled: the button only aims the composer (§4.4). Sending is the chat
  // view's job, and it goes down the `/steer …` text channel.
  enabled: true,
  title: gapTooltip("GAP-02"),
  gap: "GAP-02",
};

const JUMP: RunActionDescriptor = {
  id: "jump",
  label: "Jump to chat",
  tone: "secondary",
  enabled: true,
};

/**
 * The pause control's label is status-derived (§3.19): `paused → "Resume"`,
 * `queued → "Start now"`, otherwise `"Pause"`.
 */
export function pauseAction(status: UiStatus): RunActionDescriptor {
  if (status === "paused") {
    return { id: "resume", label: "Resume", tone: "secondary", enabled: true };
  }
  if (status === "queued") {
    // Promoting a queued task is GAP-06; `POST /v1/tasks` never dispatches.
    return blocked("start", "Start now", "GAP-06");
  }
  return { id: "pause", label: "Pause", tone: "secondary", enabled: true };
}

/** The live action bar, left to right, exactly as §3.19 orders it. */
export function liveRunActions(status: UiStatus): RunActionDescriptor[] {
  return [
    pauseAction(status),
    STEER,
    blocked("queue", "Queue follow-up", "GAP-03"),
    JUMP,
    { id: "cancel", label: "Cancel", tone: "danger", enabled: true },
  ];
}

/** The terminal banner's two controls (§3.26). The card shows only `Re-run`. */
export function terminalRunActions(): RunActionDescriptor[] {
  return [JUMP, blocked("rerun", "Re-run", "GAP-06")];
}

/** Whichever set the status calls for. */
export function runActions(status: UiStatus): RunActionDescriptor[] {
  return status === "done" || status === "cancelled" || status === "failed"
    ? terminalRunActions()
    : liveRunActions(status);
}

/**
 * The visible footnote under the detail action group: one line per action the
 * daemon cannot perform. Hover text alone would leave the gap invisible in a
 * screenshot, and the hand-off report is built from what the UI states.
 */
export function unavailableActionNotes(
  actions: readonly RunActionDescriptor[],
): string[] {
  return actions.flatMap((action) => {
    if (action.enabled || action.gap === undefined) return [];
    const gap = GAPS[action.gap];
    return [
      `${action.label} — ${gap.missingApi}. Proposed: ${gap.proposedEndpoint}`,
    ];
  });
}

/** The design's toast copy for the three actions that really fire (§4.4). */
export function actionToast(action: RunActionId, title: string): string | null {
  switch (action) {
    case "pause":
      return `${title} paused`;
    case "resume":
      return `${title} resumed`;
    case "cancel":
      return `${title} cancelled`;
    default:
      return null;
  }
}
