/**
 * An `ArtifactCard` (DESIGN_SPEC §3.13) for a file a turn referenced.
 *
 * Two populations share this component, and only one of them fetches. A
 * history-served link (`ChatMessage.attachments`/`.artifacts`, GAP-23)
 * already carries its id, name and kind from the one history query, so it
 * renders with **no request of its own** — the badge needs nothing more. A
 * live turn's SSE `done.attachments_used` carries only the file id, so that
 * chip still calls `useFileMetadata`/`useArtifact` to learn even the
 * filename, and gets the preview body and version those calls return. `Diff`
 * opens the file panel on its Diff tab rather than drawing a patch inside a
 * transcript card; the pin always writes through `PUT /v1/artifacts/{id}/pin`
 * regardless of which population the chip came from.
 */

import { ArtifactCard } from "@/components/chat";
import { toFileKind } from "@/components/ui";
import { useArtifact, useTogglePin } from "@/hooks/useArtifacts";
import { useFileMetadata } from "@/hooks/useFiles";
import type { ArtifactKind } from "@/lib/api/artifacts";
import { useUiStore } from "@/stores/ui";

import { fileKind, fileLanguage, textPreview } from "./artifact";
import type { AttachmentInfo } from "./transcript-model";

export interface TranscriptArtifactProps {
  attachment: AttachmentInfo;
}

export function TranscriptArtifact({ attachment }: TranscriptArtifactProps) {
  // A history-served link already names its file (`filename` is never null
  // for `role='attachment'`/`role='artifact'` rows the server resolved); only
  // a bare live-turn id lacks even that. Passing `null` disables the query
  // outright, so a history chip issues no request at all.
  const knownFromServer = attachment.filename !== null;
  const metadata = useFileMetadata(knownFromServer ? null : attachment.fileId);
  const row = useArtifact(knownFromServer ? null : attachment.fileId);
  const openSidePanel = useUiStore((s) => s.openSidePanel);
  const setPanelTab = useUiStore((s) => s.setPanelTab);
  const togglePin = useTogglePin();
  const cachedPin = useUiStore((s) => s.pins[attachment.fileId]);
  const pinned = cachedPin ?? row.data?.pinned ?? false;

  const name =
    attachment.filename ?? metadata.data?.filename ?? attachment.fileId;
  const mime = attachment.mimeType ?? metadata.data?.mime_type ?? null;
  const preview = textPreview(metadata.data?.extracted_text);
  // An artifact link names its kind (GAP-23); a bare file id does not, and the
  // filename and mime type are then the only signal there is.
  const kind =
    attachment.kind === null
      ? fileKind(name, mime)
      : toFileKind(attachment.kind as ArtifactKind);

  const note = knownFromServer
    ? "Open to preview this file."
    : metadata.isLoading
      ? "Reading the file…"
      : metadata.error !== null
        ? metadata.error.message
        : "No text preview for this file.";

  return (
    <ArtifactCard
      className="mb-[6px]"
      name={name}
      kind={kind}
      language={fileLanguage(name)}
      version={row.data?.version ?? null}
      previewLines={preview.lines}
      remainingLines={preview.remaining}
      unavailableNote={note}
      pinned={pinned}
      onOpen={() => openSidePanel(attachment.fileId)}
      onTogglePin={() =>
        togglePin.mutate({ id: attachment.fileId, pinned: !pinned })
      }
      onDiff={() => {
        openSidePanel(attachment.fileId);
        setPanelTab("diff");
      }}
    />
  );
}
