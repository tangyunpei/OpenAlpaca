/**
 * "Not sent to the model" — U3's GUI half.
 *
 * An attachment the daemon could not hand to the model that answered (no
 * vision, no native document part, a text extract cut by the budget) used to
 * be silent: the placeholder went into the prompt, `attachments_used` still
 * listed the id, and the only hint the reader got was the model guessing. The
 * `done` frame now carries `attachments_skipped: [{id, reason}]` and this is
 * where the turn says so.
 *
 * It is the muted-note shape the transcript already uses for an answered
 * confirmation (§3.15), not an error banner: nothing failed — the turn ran,
 * with less than the user handed it.
 *
 * Live only. The daemon stores no skip on the message, so `GET /v1/chat/history`
 * has nothing to replay and a reload drops the note (the answer, and the
 * model's own remark about the placeholder, stay).
 */

export interface SkippedAttachment {
  /** The daemon's `file_id`. Shown when the name is not known here. */
  fileId: string;
  /**
   * The file's name, when this window is the one that uploaded it. `null` for
   * a turn sent from somewhere else, or for a window that has reloaded since.
   */
  filename: string | null;
  /** The daemon's own reason, verbatim. */
  reason: string;
}

export interface SkippedAttachmentsNoteProps {
  items: readonly SkippedAttachment[];
}

export function SkippedAttachmentsNote({ items }: SkippedAttachmentsNoteProps) {
  if (items.length === 0) return null;
  return (
    <div className="mt-[10px] rounded-xl border border-line-subtle bg-muted px-[13px] py-[11px]">
      <p className="mt-0 mb-[5px] font-mono text-2xs-plus tracking-eyebrow text-tertiary uppercase">
        Not sent to the model
      </p>
      <ul className="m-0 list-none p-0">
        {items.map((entry) => (
          <li key={entry.fileId} className="text-base-plus text-secondary">
            {entry.filename ?? entry.fileId} — {entry.reason}
          </li>
        ))}
      </ul>
    </div>
  );
}
