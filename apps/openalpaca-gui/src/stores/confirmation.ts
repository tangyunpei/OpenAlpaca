/**
 * The pending tool confirmation, published out of the chat lane (§4.4, §4.5).
 *
 * Three surfaces outside the chat view need to know that the lane is blocked:
 *
 *   `useGlobalKeys`   the last rung of the Escape ladder and the Enter approval
 *   `CommandPalette`  the conditional `Approve pending <tool>` row
 *   `NavRail`         the red lane bar on the run holding the confirmation
 *
 * The chat session owns the state; routing it through the app root as props
 * would mean lifting the whole SSE session above the lazy view boundary. This
 * one-slot registry is the seam instead: `ChatView` publishes while blocked and
 * clears on resolve or unmount, and the three consumers read.
 *
 * It is deliberately *not* part of `useUiStore`: the entries carry callbacks
 * bound to a live stream, so nothing here is persistable or restorable, and
 * mixing it into the view-state store would invite someone to try.
 */

import { useEffect } from "react";
import { create } from "zustand";

export interface PendingToolConfirmation {
  /** The tool the daemon is waiting on. */
  toolName: string;
  /** The run holding it — `null` when the mapping is unknown (GAP-07). */
  runId: string | null;
  /** `resolve("approved")`. */
  approve: () => void;
  /** `resolve("denied")`. */
  deny: () => void;
}

interface ConfirmationState {
  pending: PendingToolConfirmation | null;
  setPending: (pending: PendingToolConfirmation | null) => void;
}

export const useConfirmationStore = create<ConfirmationState>((set) => ({
  pending: null,
  setPending: (pending) => set({ pending }),
}));

/**
 * Publish the lane's pending confirmation for as long as the caller is mounted.
 *
 * The effect depends on the individual fields rather than the object, so an
 * inline literal does not republish on every render of the chat view.
 */
export function usePublishConfirmation(
  pending: PendingToolConfirmation | null,
): void {
  const toolName = pending?.toolName ?? null;
  const runId = pending?.runId ?? null;
  const approve = pending?.approve;
  const deny = pending?.deny;

  useEffect(() => {
    const setPending = useConfirmationStore.getState().setPending;
    if (toolName === null || approve === undefined || deny === undefined) {
      setPending(null);
      return;
    }
    setPending({ toolName, runId, approve, deny });
    return () => setPending(null);
  }, [toolName, runId, approve, deny]);
}
