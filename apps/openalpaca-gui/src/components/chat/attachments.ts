/**
 * The composer's draft attachments (U5) — the pure half.
 *
 * A chip is one picked file on its way to `POST /v1/files/upload`: it exists
 * from the moment it is picked, carries whatever the daemon answered, and only
 * a chip the daemon gave an id to ever reaches `POST /v1/chat`. The state
 * machine is deliberately three-valued and has no "retry": a refusal is the
 * daemon's sentence, shown as it came, and the answer to it is to pick a
 * different file.
 */

import type { AttachmentRef } from "@/lib/api/types";

/**
 * How many files one turn may carry.
 *
 * The daemon is the authority — `[upload] max_files_per_message`, default 10,
 * enforced as `400 TOO_MANY_ATTACHMENTS` — and it serves the number nowhere,
 * so this mirrors the default rather than reading it. A daemon configured
 * *lower* therefore still refuses the send, with its own sentence, which is
 * the fail-closed direction: this cap only spares the user an upload that the
 * turn would have bounced anyway.
 */
export const MAX_ATTACHMENTS = 10;

export type DraftAttachmentState = "uploading" | "ready" | "failed";

export interface DraftAttachment {
  /** Client-side identity: a React key and the handle `remove` takes. */
  key: string;
  /** The picked file's name — the daemon echoes it back unchanged. */
  name: string;
  /** Bytes, as the picker reported them. */
  size: number;
  state: DraftAttachmentState;
  /** The daemon's `file_id`, once the upload answered. */
  fileId: string | null;
  /** The daemon's own refusal, verbatim. `null` unless `state === "failed"`. */
  error: string | null;
}

/**
 * The sentence for an 11th file — the daemon's own, word for word
 * (`routes/chat.rs`'s `TOO_MANY_ATTACHMENTS`), so the UI's refusal and the
 * wire's refusal read the same.
 */
export function tooManyAttachmentsMessage(attempted: number): string {
  return `Too many attachments: ${attempted} provided, maximum is ${MAX_ATTACHMENTS}`;
}

/** Only what the daemon holds travels; an upload in flight or refused does not. */
export function readyAttachmentRefs(
  items: readonly DraftAttachment[],
): AttachmentRef[] {
  const refs: AttachmentRef[] = [];
  for (const item of items) {
    if (item.state === "ready" && item.fileId !== null) {
      refs.push({ file_id: item.fileId });
    }
  }
  return refs;
}

/** True while any chip is still uploading — the composer's Send is off then. */
export function hasUploadInFlight(items: readonly DraftAttachment[]): boolean {
  return items.some((item) => item.state === "uploading");
}
