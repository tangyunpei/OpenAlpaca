/**
 * The chat view's conversation list (plan §5.7, "GUI: session sidebar").
 *
 * Migration 039 made a lane hold many conversations with exactly one `active`
 * at a time, and this is the surface that makes that visible: the live one at
 * the top, the archived ones below it in `updated_at` order, each with the
 * project it is bound to. `New chat` opens one; clicking a row resumes it.
 *
 * Purely presentational. Every verb is a callback, every state — loading,
 * failed, empty — is a prop, and nothing here invents a row: a list that could
 * not be read says so instead of rendering as "no conversations", because the
 * conversations very likely exist.
 *
 * Two rules it displays rather than implements:
 *   * **R48** — changing the window's project starts a new conversation, and
 *     the *daemon* does that on the turn itself. This list only refreshes.
 *   * **R49** — a turn addressed at a named conversation runs in *that*
 *     conversation's project, whatever the window is pointed at. When the two
 *     differ, the selected row says which one wins; a window's project
 *     indicator that quietly did not apply would be worse than no indicator.
 */

import { useState } from "react";

import { Tag } from "@/components/ui";
import type { Session } from "@/lib/api/types";
import { cn } from "@/lib/cn";

import { formatDayMonth } from "./format";

export interface SessionSidebarProps {
  /** Already ordered: the live conversation first, then archived by recency. */
  sessions: readonly Session[];
  /** The conversation the composer is addressing; `null` = the lane's active one. */
  selectedId: string | null;
  loading: boolean;
  /** The list could not be read. Rows may still exist. */
  error: Error | null;
  /** The row a write is in flight for. */
  busyId: string | null;
  creating: boolean;
  /** The last refusal, already turned into a sentence. */
  actionError: string | null;
  /**
   * The **canonical** project root this window resolves to, for the R49 line —
   * `GET /v1/status`'s `project_root`, so the comparison below is root against
   * root (R50). `null` while it is unknown, and the line then stays away.
   */
  windowProject: string | null;
  /** From `stores/pane-widths`, like the app's other three resizable columns. */
  width: number;
  /** Conversations fetched so far. */
  loaded: number;
  /** Conversations the daemon says match the query (`SessionsResponse.total`). */
  total: number;
  loadingMore: boolean;
  onLoadMore: () => void;
  /** `›` — the column is one of four in a budgeted window, so it collapses. */
  onCollapse: () => void;
  onNewChat: () => void;
  onSelect: (id: string) => void;
  onRename: (id: string, title: string) => void;
  onArchive: (id: string) => void;
  onDelete: (id: string) => void;
}

/** `/Users/dev/openalpaca` → `openalpaca`; a session with none says so. */
function projectLabel(workspaceId: string | null): string {
  if (workspaceId === null) return "no project";
  const parts = workspaceId.split(/[\\/]/).filter((part) => part.length > 0);
  return parts.at(-1) ?? workspaceId;
}

const verb = cn(
  "cursor-pointer border-none bg-transparent p-0 font-mono text-2xs tracking-label text-muted-fg uppercase",
  "transition-colors duration-[120ms] hover:text-ink",
  "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
  "disabled:pointer-events-none disabled:opacity-55",
);

export function SessionSidebar({
  sessions,
  selectedId,
  loading,
  error,
  busyId,
  creating,
  actionError,
  windowProject,
  width,
  loaded,
  total,
  loadingMore,
  onLoadMore,
  onCollapse,
  onNewChat,
  onSelect,
  onRename,
  onArchive,
  onDelete,
}: SessionSidebarProps) {
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [draftTitle, setDraftTitle] = useState("");
  /** Delete is irreversible and the daemon offers no undo, so it is two-step. */
  const [confirmingId, setConfirmingId] = useState<string | null>(null);

  function startRename(session: Session): void {
    setConfirmingId(null);
    setRenamingId(session.id);
    setDraftTitle(session.title);
  }

  function commitRename(id: string): void {
    const title = draftTitle.trim();
    setRenamingId(null);
    if (title === "") return;
    onRename(id, title);
  }

  return (
    <aside
      aria-label="Conversations"
      style={{ width }}
      className="flex shrink-0 flex-col border-r border-line-strong bg-canvas"
    >
      <div className="flex h-[46px] shrink-0 items-center gap-[10px] border-b border-line-strong px-[14px]">
        <span className="font-mono text-2xs tracking-label text-muted-fg uppercase">
          Conversations
        </span>
        <button
          type="button"
          disabled={creating}
          onClick={onNewChat}
          className={cn(verb, "ml-auto")}
        >
          {creating ? "opening…" : "New chat"}
        </button>
        <button
          type="button"
          aria-label="Collapse conversations"
          title="Collapse conversations"
          onClick={onCollapse}
          className={verb}
        >
          ›
        </button>
      </div>

      <div className="sc min-h-0 flex-1 overflow-y-auto px-[10px] py-[10px]">
        {actionError !== null && (
          <p
            role="alert"
            className="mb-[10px] font-mono text-2xs-plus text-red-ink"
          >
            {actionError}
          </p>
        )}

        {loading ? (
          <p className="font-mono text-2xs-plus text-faint">
            Loading conversations…
          </p>
        ) : error !== null ? (
          <p className="font-mono text-2xs-plus text-faint">
            Could not read this lane&apos;s conversations: {error.message}
          </p>
        ) : sessions.length === 0 ? (
          <p className="font-mono text-2xs-plus text-faint">
            No conversations yet.
          </p>
        ) : (
          <ul className="m-0 flex list-none flex-col gap-[2px] p-0">
            {sessions.map((session) => {
              const selected = session.id === selectedId;
              const busy = session.id === busyId;
              const overridesProject =
                selected &&
                session.workspace_id !== null &&
                windowProject !== null &&
                session.workspace_id !== windowProject;

              return (
                <li key={session.id} className="flex flex-col gap-[3px]">
                  {renamingId === session.id ? (
                    <form
                      onSubmit={(event) => {
                        event.preventDefault();
                        commitRename(session.id);
                      }}
                    >
                      <input
                        aria-label="Conversation title"
                        autoFocus
                        value={draftTitle}
                        onChange={(event) => setDraftTitle(event.target.value)}
                        onKeyDown={(event) => {
                          if (event.key === "Escape") setRenamingId(null);
                        }}
                        className="w-full rounded-md border border-line-strong bg-main px-[7px] py-[5px] text-base text-ink focus-visible:outline-2 focus-visible:outline-offset-0 focus-visible:outline-blue"
                      />
                    </form>
                  ) : (
                    <button
                      type="button"
                      aria-current={selected ? "true" : undefined}
                      onClick={() => onSelect(session.id)}
                      className={cn(
                        "flex w-full cursor-pointer flex-col items-start gap-[2px] rounded-md border-none px-[7px] py-[6px] text-left",
                        "transition-[background-color] duration-[120ms]",
                        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
                        selected
                          ? "bg-rail text-ink"
                          : "bg-transparent text-secondary hover:bg-muted-2",
                      )}
                    >
                      <span className="w-full truncate text-base-plus">
                        {session.title === ""
                          ? "Untitled conversation"
                          : session.title}
                      </span>
                      <span className="w-full truncate font-mono text-2xs text-faint">
                        {projectLabel(session.workspace_id)} ·{" "}
                        {session.message_count} messages ·{" "}
                        {formatDayMonth(session.updated_at)}
                      </span>
                      {(session.status === "archived" ||
                        session.interrupted_task_count > 0) && (
                        <span className="flex gap-[4px] pt-[2px]">
                          {session.status === "archived" && (
                            <Tag value="archived" />
                          )}
                          {session.interrupted_task_count > 0 && (
                            <Tag
                              tone="warn"
                              value={`${session.interrupted_task_count} interrupted`}
                            />
                          )}
                        </span>
                      )}
                    </button>
                  )}

                  {overridesProject && (
                    <p className="px-[7px] font-mono text-2xs text-amber-ink">
                      This conversation runs in {session.workspace_id}, not the
                      window&apos;s project ({windowProject}).
                    </p>
                  )}

                  <div className="flex gap-[10px] px-[7px] pb-[4px]">
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => startRename(session)}
                      className={verb}
                    >
                      Rename
                    </button>
                    {session.status === "active" && (
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => {
                          setConfirmingId(null);
                          onArchive(session.id);
                        }}
                        className={verb}
                      >
                        Archive
                      </button>
                    )}
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => {
                        if (confirmingId === session.id) {
                          setConfirmingId(null);
                          onDelete(session.id);
                        } else {
                          setConfirmingId(session.id);
                        }
                      }}
                      className={cn(
                        verb,
                        confirmingId === session.id && "text-red-ink",
                      )}
                    >
                      {confirmingId === session.id
                        ? "Confirm delete"
                        : "Delete"}
                    </button>
                  </div>
                </li>
              );
            })}
          </ul>
        )}

        {/* The daemon says how many conversations match; a list that stopped
            at one page without saying so would look complete when it is not.
            The figures are about the *fetch* — `total` is the query's count,
            before this window's lane filter — so the copy says "loaded", not
            "conversations". */}
        {loaded < total && (
          <div className="mt-[12px] flex flex-col items-start gap-[3px]">
            <button
              type="button"
              disabled={loadingMore}
              onClick={onLoadMore}
              className={verb}
            >
              {loadingMore ? "loading…" : "Show more"}
            </button>
            <span className="font-mono text-2xs text-faint">
              {loaded} of {total} loaded
            </span>
          </div>
        )}
      </div>
    </aside>
  );
}
