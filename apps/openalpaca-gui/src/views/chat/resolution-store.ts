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

import type { ResolutionEntry } from "./transcript-model";

interface ResolutionState {
  rows: ResolutionEntry[];
  /** Record one resolution. A request id is answered once, so this dedupes. */
  add: (entry: ResolutionEntry) => void;
  /**
   * G6 — an approved row's note said "waiting" until the tool reported. The
   * `tool_executed` frame names the tool, not the request, so every approved
   * row still waiting on that tool takes the finished note.
   */
  noteToolRun: (toolName: string, note: string) => void;
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

  noteToolRun: (toolName, note) =>
    set((state) => ({
      rows: state.rows.map((row) =>
        row.resolution === "approved" &&
        row.note.startsWith(waitingNotePrefix(toolName))
          ? { ...row, note }
          : row,
      ),
    })),

  clear: () => set({ rows: [] }),
}));
