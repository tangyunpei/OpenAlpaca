/**
 * The aside's file-panel mode (DESIGN_SPEC §3.23), wired to real files.
 *
 * `panelArtifactId` is a `FileAsset` id — the only artifact identity the
 * daemon has (GAP-04). Everything shown here comes from
 * `GET /v1/files/{id}`: filename, mime type and `extracted_text`.
 *
 * The three unbacked surfaces render the design's own shell with an honest
 * note rather than invented content:
 *   * the artifact switcher would list the Library — GAP-04, no list route;
 *   * `Diff` and `History` — GAP-05, nothing versioned exists in storage;
 *   * `Library ↗` still navigates, because the Library view is real chrome
 *     even while its data is not.
 */

import { FilePanel } from "@/components/chat";
import { ArtifactPreview } from "@/components/work/preview";
import { useArtifacts } from "@/hooks/useUnbacked";
import { useFileMetadata } from "@/hooks/useFiles";
import { GAPS, gapNote } from "@/lib/unavailable";
import { useUiStore } from "@/stores/ui";

import { fileKind, fileLanguage } from "./artifact";

export interface FilePanelSlotProps {
  artifactId: string;
}

export function FilePanelSlot({ artifactId }: FilePanelSlotProps) {
  const metadata = useFileMetadata(artifactId);
  const library = useArtifacts();

  const panelTab = useUiStore((s) => s.panelTab);
  const setPanelTab = useUiStore((s) => s.setPanelTab);
  const pickerOpen = useUiStore((s) => s.pickerOpen);
  const togglePicker = useUiStore((s) => s.togglePicker);
  const closePicker = useUiStore((s) => s.closePicker);
  const pickPanelArtifact = useUiStore((s) => s.pickPanelArtifact);
  const backToWork = useUiStore((s) => s.backToWork);
  const closePanel = useUiStore((s) => s.closePanel);
  const openInLibrary = useUiStore((s) => s.openInLibrary);
  const togglePin = useUiStore((s) => s.togglePin);
  const pinned = useUiStore((s) => s.pins[artifactId] === true);

  const file = metadata.data ?? null;
  const name = file?.filename ?? null;

  const artifact =
    file === null || name === null
      ? null
      : {
          id: artifactId,
          name,
          kind: fileKind(name, file.mime_type),
          language: fileLanguage(name),
          // GAP-05: nothing versioned exists, so no version is claimed.
          version: null,
          agent: null,
          runId: null,
        };

  const artifactNote =
    artifact !== null
      ? null
      : metadata.isLoading
        ? "Reading the file…"
        : metadata.error !== null
          ? metadata.error.message
          : gapNote(GAPS["GAP-04"]);

  return (
    <FilePanel
      artifact={artifact}
      artifactNote={artifactNote}
      tab={panelTab}
      onTabChange={setPanelTab}
      pickerOpen={pickerOpen}
      onTogglePicker={togglePicker}
      onClosePicker={closePicker}
      pickerItems={[]}
      pickerNote={library.available ? null : library.reason}
      onPickArtifact={pickPanelArtifact}
      onBackToWork={backToWork}
      onClose={closePanel}
      onOpenInLibrary={openInLibrary}
      pinned={pinned}
      onTogglePin={() => togglePin(artifactId)}
      previewNote={
        file === null
          ? gapNote(GAPS["GAP-04"])
          : "This file has no extracted text to preview"
      }
      diffNote={gapNote(GAPS["GAP-05"])}
      historyNote={gapNote(GAPS["GAP-05"])}
      preview={
        artifact === null ? undefined : (
          <ArtifactPreview
            size="compact"
            meta={{
              name: artifact.name,
              kind: artifact.kind,
              language: artifact.language,
            }}
            content={file?.extracted_text ?? null}
            note={
              file?.extracted_text == null
                ? "This file has no extracted text to preview"
                : null
            }
          />
        )
      }
    />
  );
}
