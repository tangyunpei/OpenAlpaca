/**
 * An `ArtifactCard` (DESIGN_SPEC §3.13) wired to a real file.
 *
 * The only artifacts the daemon can actually name today are the files a turn
 * referenced: `ChatMessage.attachments` (filename and mime inline) and the SSE
 * `done.attachments_used` ids, both resolvable through `GET /v1/files/{id}`.
 * The preview body is that file's `extracted_text` — never invented lines.
 *
 * Two card affordances have no backing API and say so instead of pretending:
 * the version chip and `Diff` are GAP-05, and the pin is client-side by design
 * (GAP-12).
 */

import { ArtifactCard } from "@/components/chat";
import { useFileMetadata } from "@/hooks/useFiles";
import { GAPS, gapNote } from "@/lib/unavailable";
import { useUiStore } from "@/stores/ui";

import { fileKind, fileLanguage, textPreview } from "./artifact";
import type { AttachmentInfo } from "./transcript-model";

export interface TranscriptArtifactProps {
  attachment: AttachmentInfo;
}

export function TranscriptArtifact({ attachment }: TranscriptArtifactProps) {
  const metadata = useFileMetadata(attachment.fileId);
  const openSidePanel = useUiStore((s) => s.openSidePanel);
  const togglePin = useUiStore((s) => s.togglePin);
  const showToast = useUiStore((s) => s.showToast);
  const pinned = useUiStore((s) => s.pins[attachment.fileId] === true);

  const name =
    attachment.filename ?? metadata.data?.filename ?? attachment.fileId;
  const mime = attachment.mimeType ?? metadata.data?.mime_type ?? null;
  const preview = textPreview(metadata.data?.extracted_text);

  const note = metadata.isLoading
    ? "Reading the file…"
    : metadata.error !== null
      ? metadata.error.message
      : "No text preview for this file.";

  return (
    <ArtifactCard
      className="mb-[6px]"
      name={name}
      kind={fileKind(name, mime)}
      language={fileLanguage(name)}
      version={null}
      previewLines={preview.lines}
      remainingLines={preview.remaining}
      unavailableNote={note}
      pinned={pinned}
      onOpen={() => openSidePanel(attachment.fileId)}
      onTogglePin={() => togglePin(attachment.fileId)}
      onDiff={() => showToast(gapNote(GAPS["GAP-05"]))}
    />
  );
}
