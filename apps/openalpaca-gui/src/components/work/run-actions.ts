/**
 * The run action catalogue (DESIGN_SPEC §3.19 action bar, §3.26 action group).
 *
 * Every one of the design's seven verbs is real now. `Start now` and `Re-run`
 * were the last two to arrive (Phase 5): what used to be GAP-06 — the action
 * route accepted exactly `cancel`, `pause`, `resume`, and nothing dispatched a
 * stored row — is served by `POST /v1/tasks/{id}/action {"action":"start"}`,
 * which runs a queued row under its own id (D5), and by
 * `POST /v1/tasks/{id}/rerun`, which copies a finished run's goal onto a new
 * one. Neither control carries a gap.
 *
 * `Queue follow-up` left that list with Phase 5. Like `Steer`, the button
 * itself only aims the composer (§4.4) — the send behind it is
 * `POST /v1/lanes/{lane_key}/followups`, which parks the text on the lane for
 * the daemon to run when the current workflow finalizes. What used to be
 * GAP-03 (storage and a `followup_queued` event, with no route to reach
 * either) is served, so the control carries no gap.
 *
 * `Steer` was the awkward one and no longer is. The design's own handler
 * (`r.steer`, §4.4) sends nothing — it aims the chat composer at the run — and
 * the send behind it is now `POST /v1/tasks/{id}/steer`, which takes the run's
 * id. What used to be GAP-02 (the daemon's only channel was the chat text
 * prefix `/steer …`, which targets the lane's active workflow and cannot name
 * a run) is served, so the control carries no gap.
 *
 * It can still be *disabled*, which is a different thing from gapped: the
 * route is owner-scoped and only reaches a live workflow, so a run the daemon
 * would refuse (`steerable: false` on the row — R40) renders disabled with the
 * reason as its tooltip. That is a property of the run, not a missing API, so
 * it carries no `gap` and stays out of the unavailable-actions footnote.
 *
 * `Cancel` / `Pause` / `Resume` are real and wired to `POST /v1/tasks/{id}/action`.
 *
 * `Resume` is two controls sharing one id, because the daemon's verb is two
 * verbs sharing one word. On a **paused** run it is the plain transition back
 * to running, on the live action bar, as it has always been. On an
 * **interrupted** one it is §5.6c's replay resume: the daemon rebuilds the
 * run's loop history from its session log and continues it under the same id.
 * That second one is experimental and off by default, so it appears on the
 * terminal banner only when `GET /v1/status` reports
 * `routing.resume_enabled` — the flag lives in the daemon's `daemon.toml`, so
 * guessing here would mean a button whose only possible answer is
 * `409 RESUME_DISABLED`. It is a property of the daemon, not a missing API, so
 * like `steerable` it carries no `gap`.
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

const STEER: RunActionDescriptor = {
  id: "steer",
  label: "Steer",
  tone: "secondary",
  // The button only aims the composer (§4.4). Sending is the chat view's job,
  // and it now POSTs to the run's own `/steer` route.
  enabled: true,
};

/**
 * `Steer`, disabled with `reason` as its tooltip when the run cannot take a
 * message — `steerDisabledReason(run)` (`run-model.ts`) is what produces it
 * from the daemon's `steerable` hint.
 */
export function steerAction(reason: string | null = null): RunActionDescriptor {
  if (reason === null) return STEER;
  return { ...STEER, enabled: false, title: reason };
}

/**
 * `Queue follow-up` — the composer's other aiming mode (§4.4).
 *
 * Enabled on every live run: unlike `Steer`, queueing does not need the
 * workflow to be listening. The item lands on the *lane*, and the daemon
 * claims it when whatever is running finishes — so a run that has stopped
 * taking steering messages can still have work parked behind it.
 */
const QUEUE: RunActionDescriptor = {
  id: "queue",
  label: "Queue follow-up",
  tone: "secondary",
  enabled: true,
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
    // D5 — the dispatch keeps the row's id, so the card the user is looking at
    // is the run that starts.
    return {
      id: "start",
      label: "Start now",
      tone: "secondary",
      enabled: true,
    };
  }
  return { id: "pause", label: "Pause", tone: "secondary", enabled: true };
}

/**
 * The live action bar, left to right, exactly as §3.19 orders it.
 *
 * `steerReason` is the run's own answer to "why not?" — `null` (the default)
 * leaves `Steer` enabled.
 */
export function liveRunActions(
  status: UiStatus,
  steerReason: string | null = null,
): RunActionDescriptor[] {
  return [
    pauseAction(status),
    steerAction(steerReason),
    QUEUE,
    JUMP,
    { id: "cancel", label: "Cancel", tone: "danger", enabled: true },
  ];
}

/**
 * The terminal banner's controls (§3.26). The card shows only `Re-run`.
 *
 * `resumable` adds §5.6c's `Resume` ahead of it, and is deliberately two
 * conditions rather than one: the run has to be `interrupted` — the only
 * status the daemon will replay — *and* this daemon has to have the
 * experimental flag on (`GET /v1/status`'s `routing.resume_enabled`).
 * Offering it otherwise would put a control on the card whose only possible
 * answer is `409 RESUME_DISABLED`, and hiding it when it works would leave a
 * recovered transcript unreachable. `Re-run` stays beside it either way — it
 * is the fallback every resume refusal points back at.
 */
export function terminalRunActions(resumable = false): RunActionDescriptor[] {
  const rerun: RunActionDescriptor = {
    id: "rerun",
    label: "Re-run",
    tone: "secondary",
    enabled: true,
  };
  if (!resumable) return [JUMP, rerun];
  return [
    JUMP,
    { id: "resume", label: "Resume", tone: "secondary", enabled: true },
    rerun,
  ];
}

/** Whichever set the status calls for. */
export function runActions(
  status: UiStatus,
  steerReason: string | null = null,
  resumeEnabled = false,
): RunActionDescriptor[] {
  // `interrupted` (§5.6b) is terminal, so it gets the terminal bar — and that
  // bar's `Re-run` is exactly the restart the daemon allows: `start` refuses a
  // finished row (R43), `rerun` copies the goal onto a new id. §5.6c adds
  // `Resume` there, and only there: a run that *chose* to stop has nothing to
  // continue from.
  return status === "done" ||
    status === "cancelled" ||
    status === "failed" ||
    status === "interrupted"
    ? terminalRunActions(status === "interrupted" && resumeEnabled)
    : liveRunActions(status, steerReason);
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

/**
 * The design's toast copy for the actions that really fire (§4.4).
 *
 * `rerun` is absent on purpose: it produces a run the user has not seen, and
 * the toast for it names that run's id, which only the caller holding the
 * response knows (`useRunController`).
 */
export function actionToast(action: RunActionId, title: string): string | null {
  switch (action) {
    case "pause":
      return `${title} paused`;
    case "resume":
      return `${title} resumed`;
    case "cancel":
      return `${title} cancelled`;
    case "start":
      return `${title} started`;
    default:
      return null;
  }
}
