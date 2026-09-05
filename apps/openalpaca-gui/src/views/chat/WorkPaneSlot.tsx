/**
 * The aside's Work-pane seam (DESIGN_SPEC §2.2, §8.4).
 *
 * The aside is **one slot with two modes** — the Work pane and the file panel
 * are never both mounted. This module owns the Work half of that slot without
 * owning the pane itself: §3.18's `WorkPane` belongs to the Work chunk, and its
 * props were shaped to be exactly `WorkPaneSlotProps`, so the default renderer
 * is a spread.
 *
 * `renderWorkPane` stays an override on `ChatView` for tests and for any host
 * that wants a different pane in that column.
 *
 * Both fields of the contract exist because only the chat lane knows them:
 * resolving a confirmation flips the blocked lane bars from red to green, drops
 * their hatched pending overlay, and turns the run-card note dot green — one
 * state change, three visual consequences (§4.4).
 */

import { WorkPane } from "@/components/work";

export interface WorkPaneSlotProps {
  /** True while a tool confirmation is pending in this lane. */
  blocked: boolean;
  /** The run holding it, or `null` when the mapping is unknown. */
  blockedRunId: string | null;
  /** `Full view` — switches to the Work view. */
  onFullView: () => void;
  /** `›` — collapses the aside entirely. */
  onCollapse: () => void;
}

export type WorkPaneRenderer = (props: WorkPaneSlotProps) => React.ReactNode;

/** The default occupant of the slot. */
export const renderDefaultWorkPane: WorkPaneRenderer = (props) => (
  <WorkPane {...props} />
);
