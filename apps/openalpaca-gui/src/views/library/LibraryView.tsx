/**
 * The Library view (DESIGN_SPEC §2.4, §5.3).
 *
 * Two columns with the third resizer between them (`libListW`, 326 / 260–480,
 * drag direction +1). The list is `GET /v1/artifacts`, paged, on one side —
 * by-products by default (`?in_project=false`), the files agents wrote into a
 * project behind `Results in the project` (`?in_project=true`, storage design
 * §13); the kind chips and the selection are client state. Nothing on screen
 * is invented: an empty list is the daemon's own empty list, and a failed one
 * says what failed.
 */

import { Resizer } from "@/components/shell";
import { useArtifactFeed } from "@/hooks/useArtifacts";
import { useUiStore } from "@/stores/ui";

import { LibraryDetail } from "./LibraryDetail";
import { LibraryList } from "./LibraryList";

export default function LibraryView() {
  const width = useUiStore((s) => s.paneWidths.libListW);
  const kind = useUiStore((s) => s.libraryKind);
  const setKind = useUiStore((s) => s.setLibraryKind);
  const results = useUiStore((s) => s.libraryResults);
  const setResults = useUiStore((s) => s.setLibraryResults);
  const openArtifactId = useUiStore((s) => s.openArtifactId);
  const openArtifact = useUiStore((s) => s.openArtifact);
  const pins = useUiStore((s) => s.pins);

  // The results side lists a file the daemon marked missing too, tagged:
  // a read that finds it again is what clears the mark (R37, d7-app-1).
  const feed = useArtifactFeed({
    inProject: results,
    includeMissing: results || undefined,
  });

  return (
    <section aria-label="Library" className="flex min-w-0 flex-1 bg-main">
      <LibraryList
        width={width}
        kind={kind}
        onKindChange={setKind}
        results={results}
        onResultsChange={setResults}
        artifacts={feed.artifacts}
        total={feed.total}
        loading={feed.loading}
        error={feed.error}
        selectedId={openArtifactId}
        onSelect={openArtifact}
        pins={pins}
        hasMore={feed.hasMore}
        loadingMore={feed.loadingMore}
        onLoadMore={feed.loadMore}
      />
      <Resizer paneKey="libListW" direction={1} label="library list" />
      <LibraryDetail artifactId={openArtifactId} />
    </section>
  );
}
