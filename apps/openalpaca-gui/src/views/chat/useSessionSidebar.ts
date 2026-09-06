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
 *   * **It never pins the conversation it creates.** A pin is a deliberate
 *     resume — a row the user clicked — and it lasts only while the daemon
 *     still calls that row active. "New chat" therefore pins nothing, and a
 *     conversation archived elsewhere releases the pin rather than sending the
 *     next turn at an archived session.
 *
 * Ordering is the controller's: the live conversation first, then the archived
 * ones by `updated_at`. The daemon already sorts `updated_at DESC, id DESC`,
 * so this is a stable partition of that order, not a re-sort.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useDaemonStatus } from "@/hooks/useConnection";
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

/** One page of the sidebar; the daemon's own default is 50. */
const SESSION_PAGE = 100;

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
  /** Conversations fetched so far, and how many the query matches in all. */
  loaded: number;
  total: number;
  loadingMore: boolean;
  loadMore: () => void;
  /**
   * The **canonical** project root this window resolves to — `GET /v1/status`'s
   * `project_root` for the picker's path, never the picker's path itself
   * (R50). `null` while it is unknown, or when the window has no project.
   */
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
  /**
   * How many pages have been asked for. The request is one widening `limit`
   * rather than an accumulating offset: the daemon orders by `updated_at DESC`
   * and writes move rows, so stitching independent offset pages together would
   * drop and duplicate conversations across the seam. One page of 100 is the
   * whole sidebar for almost everyone; a lane with more can reach the rest.
   */
  const [pages, setPages] = useState(1);
  const list = useSessions({ source: GUI_SOURCE, limit: SESSION_PAGE * pages });
  const create = useCreateSession();
  const activate = useActivateSession();
  const archiveMutation = useArchiveSession();
  const update = useUpdateSession();
  const remove = useDeleteSession();

  const projectPath = useProjectStore((s) => s.path);
  const selectedId = useSessionSelection((s) => s.selectedId);
  const select = useSessionSelection((s) => s.select);

  /**
   * R50: the R49 line compares two **canonical project roots**, so the window's
   * half has to be the daemon's answer, not the picker's raw text.
   *
   * `session.workspace_id` is what `MemoryScopeContext::for_request` resolved
   * — a marker walk up to a `.git`/`.openalpaca` root, with symlinks
   * canonicalized. `useProjectStore.path` is free text validated only for
   * absoluteness. Comparing the two announced an override for every window
   * pointed at a subdirectory of its own project (`/repo/apps/gui` vs `/repo`)
   * or at an uncanonicalized path (`/tmp/...` vs `/private/tmp/...`) — a claim
   * of a scope change in exactly the case where the daemon is provably not
   * making one. `GET /v1/status` answers the same question a turn asks, with
   * the same header, so its `project_root` is the value to compare.
   */
  const status = useDaemonStatus(projectPath);
  const windowProject = status.data?.project_root ?? null;

  const [busyId, setBusyId] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const loadMore = useCallback(() => setPages((asked) => asked + 1), []);

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

  /**
   * A pin lasts exactly as long as the daemon still calls that conversation
   * the lane's active one.
   *
   * The lane holds one active conversation, and plenty of things step it down
   * without this window doing anything: another window's "New chat", a CLI
   * `--resume`, the follow-up autostart's `claim_next`. Left pinned, the next
   * turn names an archived session and comes back `409 SESSION_ARCHIVED` for a
   * user who did nothing but type. Released, the pointer falls back to the
   * lane's active session — which is what the daemon would have chosen anyway.
   *
   * Only a row the refreshed list actually shows as non-active releases the
   * pin: a row that is merely *absent* (a later page, a filter) is unknown,
   * not archived. And neither a write in flight nor a fetch in flight can
   * release it, because the list is then still the one from before the write.
   */
  useEffect(() => {
    if (selectedId === null || busyId !== null || list.isFetching) return;
    const pinned = (list.data?.sessions ?? []).find(
      (row) => row.id === selectedId,
    );
    if (pinned === undefined || pinned.status === "active") return;
    select(null);
  }, [selectedId, busyId, list.data, list.isFetching, select]);

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

  /**
   * "New chat" creates and pins **nothing**.
   *
   * `POST /v1/sessions` archives the incumbent and makes the new row the
   * lane's active conversation, so an unpinned turn already lands in it —
   * `GET /v1/chat/history` echoes it back as `session_id` and
   * `selectedId ?? activeSessionId` highlights it. Pinning it would cost both
   * halves of the design: every later turn would name a session, so R48 could
   * never fire again, and an archive from another client would strand the
   * window on a `409`.
   */
  const newChat = useCallback(() => {
    if (create.isPending) return;
    setActionError(null);
    void create
      .mutateAsync(projectPath === null ? {} : { workspacePath: projectPath })
      .catch((error: unknown) => setActionError(sessionErrorMessage(error)));
  }, [create, projectPath]);

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
    // Both figures are the *query's*: `total` counts every `gui`-source
    // conversation the daemon holds, before this window's client-side lane
    // filter. Comparing them says whether more can be fetched, which is the
    // question the control answers.
    loaded: list.data?.sessions.length ?? 0,
    total: list.data?.total ?? 0,
    loadingMore: list.isPlaceholderData,
    loadMore,
    windowProject,
    newChat,
    select: onSelect,
    rename,
    archive,
    remove: deleteSession,
  };
}
