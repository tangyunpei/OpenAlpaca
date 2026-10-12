/**
 * The Library's left column (DESIGN_SPEC §2.4, §3.29, §3.30).
 *
 * The Library has two sides (storage design §13): by-products — drafts,
 * hand-offs, uploads, a file saved with no project — by default, and the files
 * agents wrote into a project behind the `Results in the project` chip. The
 * side is a *query* (`?in_project=`), so the header count is the daemon's own
 * unpaged `total` for that side rather than the size of this page. The kind
 * chips narrow what is *shown*: they are a view filter, not a query, because
 * two of the seven labels (`Media`, `Output`) cover more than one artifact
 * kind and `?kind=` takes exactly one — filtering here keeps every chip
 * behaving the same way and keeps `total` meaning one thing. `Load more` is
 * what walks the rest of the side.
 *
 * Presentational: the query lives in `LibraryView`, so every state below is a
 * prop and can be rendered without a daemon.
 */

import {
  KIND_FILTERS,
  KindFilterChip,
  PaneHeader,
  SectionEmpty,
  matchesKindFilter,
  toFileKind,
} from "@/components/ui";
import type { Artifact } from "@/lib/api/artifacts";

import { LibraryRow } from "./LibraryRow";

/** The chip that switches the list to the files agents wrote into a project. */
export const RESULTS_FILTER = "Results in the project";

/** The design's own empty sentence, for the by-products side. */
export const LIBRARY_EMPTY =
  "Nothing in the library yet. Files the agents produce land here.";

/** The results side's empty sentence. */
export const RESULTS_EMPTY =
  "No files written into a project yet. A file an agent writes in your project's folder is listed here.";

export interface LibraryListProps {
  width: number;
  kind: string;
  onKindChange: (kind: string) => void;
  /** The side shown: `false` by-products (the default), `true` results. */
  results: boolean;
  onResultsChange: (results: boolean) => void;
  /** Every row loaded so far, in server order. */
  artifacts: readonly Artifact[];
  /** The unpaged count, or `null` while it is unknown. */
  total: number | null;
  loading: boolean;
  error: Error | null;
  selectedId: string | null;
  onSelect: (artifactId: string) => void;
  /** The optimistic pin cache; a row falls back to the server's own field. */
  pins: Record<string, boolean>;
  hasMore: boolean;
  loadingMore: boolean;
  onLoadMore: () => void;
}

export function LibraryList({
  width,
  kind,
  onKindChange,
  results,
  onResultsChange,
  artifacts,
  total,
  loading,
  error,
  selectedId,
  onSelect,
  pins,
  hasMore,
  loadingMore,
  onLoadMore,
}: LibraryListProps) {
  const rows = artifacts.filter((artifact) =>
    matchesKindFilter(kind, toFileKind(artifact.kind)),
  );

  return (
    <div
      style={{ width }}
      className="flex shrink-0 flex-col border-r border-line-subtle"
    >
      <PaneHeader
        title="Library"
        meta={total === null ? undefined : `${total} files`}
      />

      <div className="flex flex-wrap gap-[5px] border-b border-line-hair px-[14px] pt-[12px] pb-[8px]">
        {KIND_FILTERS.map((label) => (
          <KindFilterChip
            key={label}
            label={label}
            selected={kind === label}
            onSelect={onKindChange}
          />
        ))}
        <KindFilterChip
          label={RESULTS_FILTER}
          selected={results}
          onSelect={() => onResultsChange(!results)}
          className="ml-auto"
        />
      </div>

      <div className="sc min-h-0 flex-1 overflow-y-auto p-[8px]">
        {error !== null ? (
          <SectionEmpty note={error.message}>
            The library could not be loaded.
          </SectionEmpty>
        ) : loading && artifacts.length === 0 ? (
          <SectionEmpty>Loading the library…</SectionEmpty>
        ) : rows.length === 0 ? (
          <SectionEmpty>
            {artifacts.length === 0
              ? results
                ? RESULTS_EMPTY
                : LIBRARY_EMPTY
              : `No ${kind.toLowerCase()} files among the ${artifacts.length} loaded.`}
          </SectionEmpty>
        ) : (
          rows.map((artifact) => (
            <LibraryRow
              key={artifact.id}
              artifact={artifact}
              active={artifact.id === selectedId}
              pinned={pins[artifact.id] ?? artifact.pinned}
              onSelect={onSelect}
            />
          ))
        )}

        {hasMore && error === null && (
          <button
            type="button"
            onClick={onLoadMore}
            disabled={loadingMore}
            className="mt-[6px] w-full cursor-pointer rounded-xl border border-line bg-transparent px-[10px] py-[8px] font-mono text-2xs-plus text-muted-fg hover:bg-muted-2 disabled:cursor-default disabled:text-faint"
          >
            {loadingMore
              ? "Loading…"
              : `Load more — ${artifacts.length} of ${total ?? artifacts.length}`}
          </button>
        )}
      </div>
    </div>
  );
}
