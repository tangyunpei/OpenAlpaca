/**
 * An `ArtifactCard` (DESIGN_SPEC §3.13) for a file a turn referenced.
 *
 * The card's identity comes from the attachment itself
 * (`ChatMessage.attachments`, or the SSE `done.attachments_used` ids) and its
 * preview body from that file's `extracted_text` — never invented lines. Its
 * version and pin come from the artifact row, which is the same record under
 * another route, and `Diff` opens the file panel on its Diff tab rather than
 * drawing a patch inside a transcript card.
 */

import { ArtifactCard } from "@/components/chat";
import { useArtifact, useTogglePin } from "@/hooks/useArtifacts";
import { useFileMetadata } from "@/hooks/useFiles";
import { useUiStore } from "@/stores/ui";

import { fileKind, fileLanguage, textPreview } from "./artifact";
import type { AttachmentInfo } from "./transcript-model";

export interface TranscriptArtifactProps {
  attachment: AttachmentInfo;
}

export function TranscriptArtifact({ attachment }: TranscriptArtifactProps) {
  const metadata = useFileMetadata(attachment.fileId);
  const row = useArtifact(attachment.fileId);
  const openSidePanel = useUiStore((s) => s.openSidePanel);
  const setPanelTab = useUiStore((s) => s.setPanelTab);
  const togglePin = useTogglePin();
  const cachedPin = useUiStore((s) => s.pins[attachment.fileId]);
  const pinned = cachedPin ?? row.data?.pinned ?? false;

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
