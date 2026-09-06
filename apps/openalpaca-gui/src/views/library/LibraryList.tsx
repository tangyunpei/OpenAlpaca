/**
 * The Library's left column (DESIGN_SPEC §2.4, §3.29, §3.30).
 *
 * The header count is the daemon's own unpaged `total`, so it is the size of
 * the library rather than of this page. The kind chips narrow what is *shown*:
 * they are a view filter, not a query, because two of the seven labels (`Media`,
 * `Output`) cover more than one artifact kind and `?kind=` takes exactly one —
 * filtering here keeps every chip behaving the same way and keeps `total`
 * meaning one thing. `Load more` is what walks the rest of the library.
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

export interface LibraryListProps {
  width: number;
  kind: string;
  onKindChange: (kind: string) => void;
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
              ? "Nothing in the library yet. Files the agents produce land here."
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
