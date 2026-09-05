/**
 * The Library's left column (DESIGN_SPEC §2.4, §3.29, §3.30).
 *
 * The header count is **omitted** while the list is unavailable rather than
 * shown as `0`: a zero is a claim about the user's library that this client
 * cannot make (API_MAP §2.3, GAP-04). The kind filter bar stays live either
 * way — it is client state, and the design draws it above the list, not inside
 * it.
 */

import {
  KIND_FILTERS,
  KindFilterChip,
  PaneHeader,
  SectionEmpty,
  matchesKindFilter,
  toFileKind,
} from "@/components/ui";
import type { ArtifactListPage } from "@/lib/api/unbacked";
import { gapDetail, type Availability } from "@/lib/unavailable";

import { LibraryRow } from "./LibraryRow";

export interface LibraryListProps {
  width: number;
  kind: string;
  onKindChange: (kind: string) => void;
  artifacts: Availability<ArtifactListPage>;
  selectedId: string | null;
  onSelect: (artifactId: string) => void;
  pins: Record<string, boolean>;
}

export function LibraryList({
  width,
  kind,
  onKindChange,
  artifacts,
  selectedId,
  onSelect,
  pins,
}: LibraryListProps) {
  const rows = artifacts.available
    ? artifacts.data.artifacts.filter((artifact) =>
        matchesKindFilter(kind, toFileKind(artifact.kind)),
      )
    : [];

  return (
    <div
      style={{ width }}
      className="flex shrink-0 flex-col border-r border-line-subtle"
    >
      <PaneHeader
        title="Library"
        meta={artifacts.available ? `${artifacts.data.total} files` : undefined}
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
        {!artifacts.available ? (
          <SectionEmpty note={gapDetail(artifacts)}>
            Nothing in the library yet. Files the agents produce land here.
          </SectionEmpty>
        ) : rows.length === 0 ? (
          <SectionEmpty>No {kind.toLowerCase()} files yet.</SectionEmpty>
        ) : (
          rows.map((artifact) => (
            <LibraryRow
              key={artifact.id}
              artifact={artifact}
              active={artifact.id === selectedId}
              pinned={pins[artifact.id] === true}
              onSelect={onSelect}
            />
          ))
        )}
      </div>
    </div>
  );
}
