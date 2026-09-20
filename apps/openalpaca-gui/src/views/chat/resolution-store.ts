/**
 * The transcript's resolution rows, for the life of the window (P2).
 *
 * `Approved` / `Denied` / `Timed out` are the record of something that
 * happened **to the person**: a prompt they answered, or one nobody did. Held
 * in `useChatSession`'s own `useState` they lasted exactly as long as the chat
 * view was mounted — and the views are swapped, not stacked (`App.tsx`'s
 * `renderView` returns a different lazy component per view), so one click on a
 * `FILE WRITTEN` card, which opens the Library, took every row on screen with
 * it. The owner's second Tauri session hit precisely that: a `TIMED OUT` row
 * on screen at 15:52 was gone afterwards, while a later run's `DENIED` row —
 * written after the trip — was still there.
 *
 * So the rows live above the view. A reload still clears them, which is right:
 * no resolution is persisted anywhere, and inventing one after a restart would
 * be worse than not showing it.
 *
 * It is a store next to the view rather than one in `stores/` because nothing
 * outside chat reads it; the reason it is a store at all is *lifetime*, not
 * sharing. The rows are still cleared when the conversation changes (I5) —
 * that is the session-local reset, and it is the hook that owns it.
 */

import { create } from "zustand";

import { sameToolOwner, type ToolOwner } from "@/components/chat";

import type { ResolutionEntry } from "./transcript-model";

interface ResolutionState {
  rows: ResolutionEntry[];
  /** Record one resolution. A request id is answered once, so this dedupes. */
  add: (entry: ResolutionEntry) => void;
  /**
   * G6 — an approved row's note said "waiting" until the tool reported. The
   * `tool_executed` frame names the tool, not the request, so an approved row
   * still waiting on that tool takes the finished note.
   *
   * F5 — the name alone is not enough. The socket carries every lane's
   * frames, so the tool a background workflow's subagent ran rewrote this
   * window's row with an outcome that was not its call's, and the real frame
   * arriving 40 s later found no row still waiting to correct. The owner is
   * the other half of the match: same tool, same agent, same run.
   */
  noteToolRun: (toolName: string, owner: ToolOwner, note: string) => void;
  /** The conversation changed; these rows belonged to the previous one. */
  clear: () => void;
}

/** The prefix an approved row wears while its tool has not reported yet. */
export function waitingNotePrefix(toolName: string): string {
  return `${toolName} approved · waiting`;
}

export const useResolutions = create<ResolutionState>((set) => ({
  rows: [],

  add: (entry) =>
    set((state) =>
      state.rows.some((row) => row.requestId === entry.requestId)
        ? state
        : { rows: [...state.rows, entry] },
    ),

  noteToolRun: (toolName, owner, note) =>
    set((state) => ({
      rows: state.rows.map((row) =>
        row.resolution === "approved" &&
        row.note.startsWith(waitingNotePrefix(toolName)) &&
        sameToolOwner(owner, row)
          ? { ...row, note }
          : row,
      ),
    })),

  clear: () => set({ rows: [] }),
}));
