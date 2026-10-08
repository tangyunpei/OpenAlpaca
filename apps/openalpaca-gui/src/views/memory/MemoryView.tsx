/**
 * The Memory view — the fifth top-level view (memory redesign, design §7
 * *Surfaces*; owner decisions G30 and MD-9).
 *
 * The Library's shape: a list column beside a detail column, the head pinned
 * and the body scrolling. The list shows one partition — **Global**, or **This
 * project** once the daemon has resolved the window's project to a root —
 * narrowed by status (active, candidates, archived) or replaced by a search;
 * above it, while nothing is searched, the **suggestions** — learned changes
 * to notes the owner saved, each with Replace and Keep mine (design 09 §3.5)
 * — and the notes learned from what the owner said, each with its one-step
 * undo. The detail reads one note and is where it is confirmed, edited,
 * archived or forgotten.
 *
 * Everything here is a user-directed surface (design §4): a write records
 * origin `user`. Nothing polls — `memory_changed` invalidates `["memory"]`.
 */

import { useState } from "react";

import { useDaemonStatus } from "@/hooks/useConnection";
import {
  useConfirmSuggestion,
  useDismissSuggestion,
  useMemoryFeed,
  useMemorySearch,
  useNoteTitles,
  useRecentlyLearned,
  useSuggestions,
  useUndoMemory,
} from "@/hooks/useMemories";
import {
  isMemoryConflict,
  type MemoryListItem,
  type MemorySuggestion,
} from "@/lib/api/memories";
import { PANE_DEFAULTS } from "@/stores/pane-widths";
import { useProjectStore } from "@/stores/project";
import { useUiStore } from "@/stores/ui";

import { AddNoteForm } from "./AddNoteForm";
import {
  KEPT_YOUR_NOTE,
  MEMORY_ACTION_CONFLICT,
  replacedToast,
  undoneToast,
} from "./copy";
import { MemoryDetail, type ForgottenNotice } from "./MemoryDetail";
import { MemoryList } from "./MemoryList";

/**
 * The window's project root as the daemon resolved it: a path; `null` when
 * there is no project or the path is inside none; `undefined` while the
 * answer is out. Only the daemon can resolve a path to a project.
 */
function useProjectRoot(projectPath: string | null): string | null | undefined {
  const status = useDaemonStatus(projectPath);
  if (projectPath === null) return null;
  if (status.data === undefined) return undefined;
  const root = status.data.project_root;
  return typeof root === "string" ? root : null;
}

export default function MemoryView() {
  const projectPath = useProjectStore((s) => s.path);
  const projectRoot = useProjectRoot(projectPath);

  const wantedScope = useUiStore((s) => s.memoryScope);
  const setScope = useUiStore((s) => s.setMemoryScope);
  const status = useUiStore((s) => s.memoryStatus);
  const setStatus = useUiStore((s) => s.setMemoryStatus);
  const openMemoryId = useUiStore((s) => s.openMemoryId);
  const openMemory = useUiStore((s) => s.openMemory);
  const showToast = useUiStore((s) => s.showToast);

  // `project` is honoured only while it means something; the chip says why not.
  const scope =
    wantedScope === "project" && typeof projectRoot === "string"
      ? "project"
      : "global";

  const [query, setQuery] = useState("");
  const [adding, setAdding] = useState(false);
  const [forgotten, setForgotten] = useState<ForgottenNotice | null>(null);
  const q = query.trim();

  const feed = useMemoryFeed(scope, status, projectPath);
  const search = useMemorySearch(q, scope, projectPath);
  const learned = useRecentlyLearned(projectPath);
  const undo = useUndoMemory(projectPath);
  const suggestions = useSuggestions(projectPath);
  const confirmSuggestion = useConfirmSuggestion(projectPath);
  const dismissSuggestion = useDismissSuggestion(projectPath);

  const learnedHere = (learned.data ?? []).filter(
    (item) => item.scope === scope,
  );
  const suggestionsHere = (suggestions.data ?? []).filter(
    (item) => item.scope === scope,
  );
  const replacedIds = [
    ...new Set(learnedHere.flatMap((item) => item.supersedes.slice(0, 1))),
  ];
  const replacedTitles = useNoteTitles(replacedIds, projectPath);

  const refused = (verb: string) => (error: Error) =>
    showToast(
      isMemoryConflict(error)
        ? MEMORY_ACTION_CONFLICT
        : `Could not ${verb} — ${error.message}`,
    );

  const onUndo = (item: MemoryListItem, replaced: string | null) => {
    undo.mutate(
      { id: item.id, revision: item.revision },
      {
        onSuccess: () => showToast(undoneToast(replaced, item.title)),
        onError: refused("undo"),
      },
    );
  };

  const onReplace = (suggestion: MemorySuggestion) => {
    confirmSuggestion.mutate(
      { suggestion },
      {
        onSuccess: () => showToast(replacedToast(suggestion.target.title)),
        onError: refused("replace"),
      },
    );
  };

  const onKeep = (suggestion: MemorySuggestion) => {
    dismissSuggestion.mutate(
      { suggestion },
      {
        onSuccess: () => showToast(KEPT_YOUR_NOTE),
        onError: refused("keep your note"),
      },
    );
  };

  const suggestionPendingId = confirmSuggestion.isPending
    ? (confirmSuggestion.variables?.suggestion.id ?? null)
    : dismissSuggestion.isPending
      ? (dismissSuggestion.variables?.suggestion.id ?? null)
      : null;

  const select = (id: string) => {
    setAdding(false);
    setForgotten(null);
    openMemory(id);
  };

  return (
    <section aria-label="Memory" className="flex min-w-0 flex-1 bg-main">
      <MemoryList
        width={PANE_DEFAULTS.libListW}
        scope={scope}
        onScopeChange={setScope}
        projectRoot={projectRoot ?? null}
        projectUnavailable={wantedScope === "project" && projectRoot === null}
        status={status}
        onStatusChange={setStatus}
        query={query}
        onQueryChange={setQuery}
        feed={feed}
        search={q === "" ? null : search}
        suggestions={q === "" && status === "active" ? suggestionsHere : []}
        suggestionPendingId={suggestionPendingId}
        onReplace={onReplace}
        onKeep={onKeep}
        learned={q === "" && status === "active" ? learnedHere : []}
        replacedTitles={replacedTitles}
        undoPendingId={undo.isPending ? (undo.variables?.id ?? null) : null}
        onUndo={onUndo}
        selectedId={adding ? null : openMemoryId}
        onSelect={select}
        onAdd={() => setAdding(true)}
      />
      {adding ? (
        <AddNoteForm
          scope={scope}
          workspacePath={projectPath}
          onCancel={() => setAdding(false)}
          onCreated={select}
        />
      ) : (
        // Keyed by the note, so a draft, an armed Forget or a chosen diff base
        // never carries from one note to the next.
        <MemoryDetail
          key={openMemoryId ?? "none"}
          noteId={openMemoryId}
          workspacePath={projectPath}
          forgotten={forgotten}
          onForgotten={setForgotten}
        />
      )}
    </section>
  );
}
