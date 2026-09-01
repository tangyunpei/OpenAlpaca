/**
 * The Library view (DESIGN_SPEC §2.4, §5.3).
 *
 * Two columns with the third resizer between them (`libListW`, 326 / 260–480,
 * drag direction +1). Everything on screen is either client state (the kind
 * filter, the selection, pins) or an honest absence: `GET /v1/artifacts` does
 * not exist (API_MAP §2.3, GAP-04), so the list renders its real chrome over
 * the adapter's unavailable state and names the route it is waiting for.
 */

import { Resizer } from "@/components/shell";
import { useArtifacts } from "@/hooks/useUnbacked";
import { useUiStore } from "@/stores/ui";

import { LibraryDetail } from "./LibraryDetail";
import { LibraryList } from "./LibraryList";

export default function LibraryView() {
  const width = useUiStore((s) => s.paneWidths.libListW);
  const kind = useUiStore((s) => s.libraryKind);
  const setKind = useUiStore((s) => s.setLibraryKind);
  const openArtifactId = useUiStore((s) => s.openArtifactId);
  const openArtifact = useUiStore((s) => s.openArtifact);
  const pins = useUiStore((s) => s.pins);

  const artifacts = useArtifacts();

  return (
    <section aria-label="Library" className="flex min-w-0 flex-1 bg-main">
      <LibraryList
        width={width}
        kind={kind}
        onKindChange={setKind}
        artifacts={artifacts}
        selectedId={openArtifactId}
        onSelect={openArtifact}
        pins={pins}
      />
      <Resizer paneKey="libListW" direction={1} label="library list" />
      <LibraryDetail artifactId={openArtifactId} />
    </section>
  );
}
