/**
 * The composer's attachments (U5) — the moving half.
 *
 * Everything a chip goes through lives here: the cap, the upload, the daemon's
 * refusal, and the `file_id → filename` table the "not sent to the model" note
 * reads (U3). `Composer` renders what this holds and nothing else.
 *
 * Two rules worth keeping:
 *   * **a file uploads the moment it is picked.** The alternative — uploading
 *     on send — makes the send slow, makes a refusal arrive when the user has
 *     already committed, and gives the chip nothing honest to say in between.
 *   * **the count is a ref, not the list's length.** Picks arrive in bursts (a
 *     multi-select, a drop), and reading a `useState` list inside the handler
 *     that just queued an update would let the 11th file through.
 */

import { useCallback, useRef, useState } from "react";

import {
  MAX_ATTACHMENTS,
  readyAttachmentRefs,
  tooManyAttachmentsMessage,
  type DraftAttachment,
} from "@/components/chat";
import { uploadErrorMessage, uploadFile } from "@/lib/api/files";
import type { AttachmentRef } from "@/lib/api/types";

let keySeq = 0;

function nextKey(): string {
  keySeq += 1;
  return `att-${keySeq}`;
}

export interface ComposerAttachments {
  items: DraftAttachment[];
  /**
   * A pick that never became a chip — today only the cap. Cleared by the next
   * successful pick and by a send.
   */
  error: string | null;
  add: (files: readonly File[]) => void;
  remove: (key: string) => void;
  /** What a send carries: the chips the daemon actually holds. */
  refs: AttachmentRef[];
  /** True while any upload is in flight — Send is off, and so is a send. */
  uploading: boolean;
  /** Drop every chip. Called when the daemon accepted the turn, never before. */
  clear: () => void;
  /**
   * `file_id → filename` for every upload this window made, so a skipped
   * attachment can be named rather than printed as an id. It outlives the
   * chips on purpose: the note is drawn after the chips are cleared.
   */
  names: Readonly<Record<string, string>>;
}

export function useComposerAttachments(
  workspacePath: string | null,
): ComposerAttachments {
  const [items, setItems] = useState<DraftAttachment[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [names, setNames] = useState<Record<string, string>>({});

  /** How many chips exist right now — updated synchronously by every mutator. */
  const count = useRef(0);
  const workspace = useRef(workspacePath);
  workspace.current = workspacePath;

  const add = useCallback((files: readonly File[]) => {
    const picked = Array.from(files);
    if (picked.length === 0) return;

    const room = Math.max(0, MAX_ATTACHMENTS - count.current);
    const taken = picked.slice(0, room);
    setError(
      taken.length < picked.length
        ? tooManyAttachmentsMessage(count.current + picked.length)
        : null,
    );
    if (taken.length === 0) return;

    count.current += taken.length;
    const drafts: DraftAttachment[] = taken.map((file) => ({
      key: nextKey(),
      name: file.name,
      size: file.size,
      state: "uploading",
      fileId: null,
      error: null,
    }));
    setItems((current) => [...current, ...drafts]);

    taken.forEach((file, index) => {
      const draft = drafts[index];
      if (draft === undefined) return;
      void uploadFile(file, { workspacePath: workspace.current })
        .then((asset) => {
          setNames((current) => ({ ...current, [asset.id]: asset.filename }));
          setItems((current) =>
            current.map((entry) =>
              entry.key === draft.key
                ? { ...entry, state: "ready", fileId: asset.id }
                : entry,
            ),
          );
        })
        .catch((cause: unknown) => {
          // The chip stays — a refusal the user cannot see is the thing U3
          // exists to stop — and it carries the daemon's own sentence.
          setItems((current) =>
            current.map((entry) =>
              entry.key === draft.key
                ? {
                    ...entry,
                    state: "failed",
                    error: uploadErrorMessage(cause),
                  }
                : entry,
            ),
          );
        });
    });
  }, []);

  const remove = useCallback((key: string) => {
    setItems((current) => {
      const next = current.filter((entry) => entry.key !== key);
      count.current = next.length;
      return next;
    });
    setError(null);
  }, []);

  const clear = useCallback(() => {
    count.current = 0;
    setItems([]);
    setError(null);
  }, []);

  return {
    items,
    error,
    add,
    remove,
    refs: readyAttachmentRefs(items),
    uploading: items.some((entry) => entry.state === "uploading"),
    clear,
    names,
  };
}
