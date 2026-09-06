/**
 * The session sidebar's glue: the lane's conversations, the five write verbs,
 * and the selection the composer addresses.
 *
 * What it deliberately does **not** do:
 *   * **It never creates a session because the project changed (R48).** That
 *     is the daemon's, on the turn itself — `resolve_turn_session` archives
 *     the incumbent and opens a new conversation when a turn's project differs
 *     from the active session's binding. A client that also created one would
 *     produce two conversations for one switch. What this hook does instead is
 *     drop the resume pointer, so the next turn names no session and R48 can
 *     fire at all; the new row arrives through the `session_changed` frame
 *     (`query-client.ts` invalidates `sessions` and `chat` on it).
 *   * **It never guesses which conversation is live.** `selectedId === null`
 *     means "the lane's active session", and the daemon answers that on every
 *     read; the highlighted row is the one `GET /v1/chat/history` reports as
 *     its `session_id`, not a local memory of what was clicked.
 *
 * Ordering is the controller's: the live conversation first, then the archived
 * ones by `updated_at`. The daemon already sorts `updated_at DESC, id DESC`,
 * so this is a stable partition of that order, not a re-sort.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  useActivateSession,
  useArchiveSession,
  useCreateSession,
  useDeleteSession,
  useSessions,
  useUpdateSession,
} from "@/hooks/useSessions";
import { sessionErrorMessage } from "@/lib/api/sessions";
import type { Session } from "@/lib/api/types";
import { useProjectStore } from "@/stores/project";
import { useSessionSelection } from "@/stores/session";

/** One page is the whole sidebar; the daemon's own default is 50. */
const SESSION_LIMIT = 100;

/** The lane the GUI talks on is `{local_user}:gui`, so the source is the filter. */
const GUI_SOURCE = "gui";

export interface SessionSidebarState {
  sessions: Session[];
  /** The resumed conversation, or the lane's active one when nothing is pinned. */
  selectedId: string | null;
  loading: boolean;
  error: Error | null;
  busyId: string | null;
  creating: boolean;
  actionError: string | null;
  windowProject: string | null;
  newChat: () => void;
  select: (id: string) => void;
  rename: (id: string, title: string) => void;
  archive: (id: string) => void;
  remove: (id: string) => void;
}

/**
 * @param laneKey the lane this window is talking on, once a turn or a history
 *   read has reported one. `null` before that, and the list is then unfiltered
 *   by lane — the `gui` source is still applied server-side.
 * @param activeSessionId the conversation the daemon says the lane's next
 *   unpinned turn lands in (`GET /v1/chat/history`'s `session_id`). It is what
 *   the sidebar highlights when nothing is explicitly resumed.
 */
export function useSessionSidebar(
  laneKey: string | null,
  activeSessionId: string | null,
): SessionSidebarState {
  const list = useSessions({ source: GUI_SOURCE, limit: SESSION_LIMIT });
  const create = useCreateSession();
  const activate = useActivateSession();
  const archiveMutation = useArchiveSession();
  const update = useUpdateSession();
  const remove = useDeleteSession();

  const projectPath = useProjectStore((s) => s.path);
  const selectedId = useSessionSelection((s) => s.selectedId);
  const select = useSessionSelection((s) => s.select);

  const [busyId, setBusyId] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  /**
   * R48 has to stay reachable. A pinned conversation makes every turn name it
   * (R49), which by construction means no project switch can ever be detected
   * — so changing the window's project releases the pin and the next turn goes
   * back to the lane's active session, where the daemon decides.
   */
  const lastProject = useRef(projectPath);
  useEffect(() => {
    if (lastProject.current === projectPath) return;
    lastProject.current = projectPath;
    select(null);
  }, [projectPath, select]);

  const sessions = useMemo(() => {
    const rows = (list.data?.sessions ?? []).filter(
      (row) => laneKey === null || row.lane_key === laneKey,
    );
    // A stable partition of the daemon's own `updated_at DESC` ordering.
    return [
      ...rows.filter((row) => row.status === "active"),
      ...rows.filter((row) => row.status !== "active"),
    ];
  }, [list.data, laneKey]);

  /** Run one write, holding the row busy and turning any refusal into a sentence. */
  const runVerb = useCallback(
    (id: string, verb: () => Promise<unknown>, after?: () => void) => {
      if (busyId !== null) return;
      setBusyId(id);
      setActionError(null);
      void verb()
        .then(() => after?.())
        .catch((error: unknown) => setActionError(sessionErrorMessage(error)))
        .finally(() => setBusyId(null));
    },
    [busyId],
  );

  const newChat = useCallback(() => {
    if (create.isPending) return;
    setActionError(null);
    void create
      .mutateAsync(projectPath === null ? {} : { workspacePath: projectPath })
      // Addressing the conversation just created keeps the window on it even
      // if another client opens one a moment later. Its binding is this
      // window's project, so R49 substitutes the same value the header would
      // have carried and nothing changes scope.
      .then((session) => select(session.id))
      .catch((error: unknown) => setActionError(sessionErrorMessage(error)));
  }, [create, projectPath, select]);

  const onSelect = useCallback(
    (id: string) => {
      const row = sessions.find((session) => session.id === id);
      if (row === undefined) return;
      // Already the lane's live conversation: pin it, but do not write. An
      // `activate` here would still archive-and-reopen the same row and
      // announce a transition that did not happen.
      if (row.status === "active") {
        setActionError(null);
        select(id);
        return;
      }
      runVerb(
        id,
        () => activate.mutateAsync(id),
        () => select(id),
      );
    },
    [sessions, select, runVerb, activate],
  );

  const rename = useCallback(
    (id: string, title: string) => {
      runVerb(id, () => update.mutateAsync({ id, title }));
    },
    [runVerb, update],
  );

  const archive = useCallback(
    (id: string) => {
      runVerb(
        id,
        () => archiveMutation.mutateAsync(id),
        () => {
          // The lane has no active session afterwards; the next turn opens one.
          if (selectedId === id) select(null);
        },
      );
    },
    [runVerb, archiveMutation, selectedId, select],
  );

  const deleteSession = useCallback(
    (id: string) => {
      runVerb(
        id,
        () => remove.mutateAsync(id),
        () => {
          if (selectedId === id) select(null);
        },
      );
    },
    [runVerb, remove, selectedId, select],
  );

  return {
    sessions,
    selectedId: selectedId ?? activeSessionId,
    loading: list.isPending,
    error: list.error,
    busyId,
    creating: create.isPending,
    actionError,
    windowProject: projectPath,
    newChat,
    select: onSelect,
    rename,
    archive,
    remove: deleteSession,
  };
}
