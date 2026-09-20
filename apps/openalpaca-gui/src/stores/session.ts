/**
 * Which conversation the composer is addressing (plan §5.7).
 *
 * `null` — the default — means **the lane's active session**, whatever that
 * currently is. It is not "no conversation": the daemon resolves the lane's
 * active session for any turn that names none, which is exactly today's
 * behaviour and the only state in which R48 can do its job (a turn whose
 * project differs from the active session's opens a new conversation).
 *
 * A non-null id is a deliberate **resume**: the user picked that row, so the
 * turn names it, and R49 then makes *that conversation's* project govern the
 * turn instead of the window's. Because the two rules pull in opposite
 * directions, the sidebar clears the pointer whenever the window's project
 * changes — see `useSessionSidebar`.
 *
 * Deliberately not persisted. §5.6(d) is explicit that the last-session
 * pointer belongs on the server (`last_session:{workspace_id}` in the
 * preference KV, P-23) precisely because a client-side one diverges between
 * two windows and a database; that pointer is not served yet, and caching a
 * guess in `localStorage` in the meantime would be inventing one.
 */

import { create } from "zustand";

export interface SessionSelectionState {
  /** The resumed conversation, or `null` for the lane's active one. */
  selectedId: string | null;
  select: (sessionId: string | null) => void;
}

export const useSessionSelection = create<SessionSelectionState>((set) => ({
  selectedId: null,
  select: (selectedId) => set({ selectedId }),
}));
