/**
 * The Memory view's left column: the scope selector, the search box, the
 * status chips, the recently-learned rows and the notes.
 *
 * Presentational: every query lives in `MemoryView`, so each state below is a
 * prop and renders without a daemon. A failed read says what failed — an
 * empty list would claim "nothing remembered", which is a different thing
 * from "this could not be read".
 */

import type { UseQueryResult } from "@tanstack/react-query";

import {
  Button,
  KindFilterChip,
  PaneHeader,
  SectionEmpty,
  chipVariant,
} from "@/components/ui";
import type { MemoryFeed } from "@/hooks/useMemories";
import type {
  MemoryListItem,
  MemorySearchPage,
  MemorySuggestion,
  NoteScope,
  NoteStatus,
} from "@/lib/api/memories";

import { MEMORY_EMPTY_COPY, scopeLabel } from "./copy";
import { MemoryRow, listRow, searchRow } from "./MemoryRow";
import { RecentlyLearned } from "./RecentlyLearned";
import { Suggestions } from "./Suggestions";

const STATUS_CHIPS: readonly { status: NoteStatus; label: string }[] = [
  { status: "active", label: "Active" },
  { status: "candidate", label: "Candidates" },
  { status: "archived", label: "Archived" },
];

/** What an empty list says, per status — only `active` is the design's. */
function emptyCopy(status: NoteStatus, scope: NoteScope): string {
  if (status === "candidate") {
    return "No candidates waiting. Things Alpaca infers rather than hears from you wait here for your confirmation.";
  }
  if (status === "archived") return "Nothing archived.";
  return scope === "project"
    ? "Nothing remembered for this project yet."
    : MEMORY_EMPTY_COPY;
}

export interface MemoryListProps {
  width: number;
  scope: NoteScope;
  onScopeChange: (scope: NoteScope) => void;
  /** The daemon's root for the window's project, or `null` when there is none. */
  projectRoot: string | null;
  /**
   * The owner chose This project and it is not available — the window has no
   * project, or its path is inside none — so Global is on screen instead.
   * Said, never silently swapped.
   */
  projectUnavailable: boolean;
  status: NoteStatus;
  onStatusChange: (status: NoteStatus) => void;
  query: string;
  onQueryChange: (query: string) => void;
  feed: MemoryFeed;
  /** The search in flight or answered; `null` while the box is empty. */
  search: UseQueryResult<MemorySearchPage> | null;
  /** Pending suggestions in this scope; `[]` hides the section. */
  suggestions?: readonly MemorySuggestion[];
  suggestionPendingId?: string | null;
  onReplace?: (suggestion: MemorySuggestion) => void;
  onKeep?: (suggestion: MemorySuggestion) => void;
  /** Recently learned in this scope; `[]` hides the section. */
  learned: readonly MemoryListItem[];
  /** Titles of the notes the learned rows replaced, by id. */
  replacedTitles?: ReadonlyMap<string, string>;
  undoPendingId: string | null;
  onUndo: (item: MemoryListItem, replaced: string | null) => void;
  selectedId: string | null;
  onSelect: (id: string) => void;
  onAdd: () => void;
}

export function MemoryList({
  width,
  scope,
  onScopeChange,
  projectRoot,
  projectUnavailable,
  status,
  onStatusChange,
  query,
  onQueryChange,
  feed,
  search,
  suggestions = [],
  suggestionPendingId = null,
  onReplace = () => {},
  onKeep = () => {},
  learned,
  replacedTitles,
  undoPendingId,
  onUndo,
  selectedId,
  onSelect,
  onAdd,
}: MemoryListProps) {
  const meta =
    search !== null
      ? search.data === undefined
        ? undefined
        : `${search.data.count} found`
      : feed.loading
        ? undefined
        : `${feed.items.length}${feed.hasMore ? "+" : ""} notes`;

  return (
    <div
      style={{ width }}
      className="flex shrink-0 flex-col border-r border-line-subtle"
    >
      <PaneHeader title="Memory" meta={meta}>
        <Button variant="primarySm" onClick={onAdd}>
          Add note
        </Button>
      </PaneHeader>

      <div className="flex flex-col gap-[8px] border-b border-line-hair px-[14px] pt-[12px] pb-[10px]">
        <div
          role="group"
          aria-label="Scope"
          className="flex items-center gap-[5px]"
        >
          {(["global", "project"] as const).map((value) => (
            <Button
              key={value}
              variant={chipVariant(scope === value)}
              aria-pressed={scope === value}
              disabled={value === "project" && projectRoot === null}
              title={
                value === "project"
                  ? (projectRoot ??
                    "Choose a project in Settings → Connection; it needs a .git or .openalpaca above it.")
                  : undefined
              }
              onClick={() => onScopeChange(value)}
            >
              {scopeLabel(value)}
            </Button>
          ))}
        </div>
        {projectUnavailable && (
          <p className="m-0 font-mono text-2xs-plus leading-[1.5] text-faint">
            This window has no project, or its path is inside none (no .git or
            .openalpaca above it), so Global is shown.
          </p>
        )}

        <input
          type="search"
          aria-label="Search memory"
          placeholder="Search notes…"
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          className="w-full rounded-md border border-line bg-raised px-[8px] py-[5px] text-base text-ink placeholder:text-faint"
        />

        {search === null && (
          <div className="flex flex-wrap gap-[5px]">
            {STATUS_CHIPS.map((chip) => (
              <KindFilterChip
                key={chip.status}
                label={chip.label}
                selected={status === chip.status}
                onSelect={() => onStatusChange(chip.status)}
              />
            ))}
          </div>
        )}
      </div>

      <div className="sc min-h-0 flex-1 overflow-y-auto p-[8px]">
        <Suggestions
          items={suggestions}
          pendingId={suggestionPendingId}
          onReplace={onReplace}
          onKeep={onKeep}
          onOpen={onSelect}
        />
        {learned.length > 0 && (
          <RecentlyLearned
            items={learned}
            pendingId={undoPendingId}
            replacedTitles={replacedTitles}
            onUndo={onUndo}
            onOpen={onSelect}
          />
        )}

        {search !== null ? (
          <SearchResults
            search={search}
            query={query}
            selectedId={selectedId}
            onSelect={onSelect}
          />
        ) : feed.error !== null ? (
          <SectionEmpty note={feed.error.message}>
            Memory could not be read.
          </SectionEmpty>
        ) : feed.loading && feed.items.length === 0 ? (
          <SectionEmpty>Reading memory…</SectionEmpty>
        ) : feed.items.length === 0 ? (
          <SectionEmpty>{emptyCopy(status, scope)}</SectionEmpty>
        ) : (
          <ul aria-label="Notes" className="m-0 list-none p-0">
            {feed.items.map((item) => (
              <li key={item.id}>
                <MemoryRow
                  row={listRow(item)}
                  active={item.id === selectedId}
                  onSelect={onSelect}
                />
              </li>
            ))}
          </ul>
        )}

        {search === null && feed.hasMore && feed.error === null && (
          <button
            type="button"
            onClick={feed.loadMore}
            disabled={feed.loadingMore}
            className="mt-[6px] w-full cursor-pointer rounded-xl border border-line bg-transparent px-[10px] py-[8px] font-mono text-2xs-plus text-muted-fg hover:bg-muted-2 disabled:cursor-default disabled:text-faint"
          >
            {feed.loadingMore ? "Loading…" : "Load more"}
          </button>
        )}
      </div>
    </div>
  );
}

function SearchResults({
  search,
  query,
  selectedId,
  onSelect,
}: {
  search: UseQueryResult<MemorySearchPage>;
  query: string;
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  if (search.error !== null) {
    return (
      <SectionEmpty note={search.error.message}>
        The search could not run.
      </SectionEmpty>
    );
  }
  const page = search.data;
  if (page === undefined) return <SectionEmpty>Searching…</SectionEmpty>;
  return (
    <>
      {page.index_state === "rebuilding" && (
        <p className="m-0 mb-[6px] px-[6px] font-mono text-2xs-plus text-faint">
          The search index is rebuilding; these come from a slower file scan.
        </p>
      )}
      {page.results.length === 0 ? (
        <SectionEmpty>No note matches “{query.trim()}”.</SectionEmpty>
      ) : (
        <ul aria-label="Notes" className="m-0 list-none p-0">
          {page.results.map((result) => (
            <li key={result.id}>
              <MemoryRow
                row={searchRow(result)}
                active={result.id === selectedId}
                onSelect={onSelect}
              />
            </li>
          ))}
        </ul>
      )}
      {page.partial && (
        <p className="m-0 mt-[6px] px-[6px] font-mono text-2xs-plus text-faint">
          The search stopped at its limit, so there may be more; a narrower
          query finds them.
        </p>
      )}
    </>
  );
}
