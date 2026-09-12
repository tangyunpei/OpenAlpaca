/**
 * The lane's pending follow-ups, above the composer.
 *
 * `Queue follow-up` parks work for after the current run; without a read-back
 * the user had to take on faith that it landed, and no way at all to take it
 * out again. This is that read-back: `GET /v1/lanes/{lane_key}/followups`,
 * oldest first — which is the order the daemon will claim them in — with a
 * `Cancel` per row.
 *
 * Two kinds share the list and they are not the same thing, so the pill says
 * which: a `follow-up` is work the user (or the model) queued and the daemon
 * will *run*; an `unprocessed_steering` row is a steering message a workflow
 * exited before reading, which is never auto-run — it is shown to the lead on
 * the lane's next turn instead. Cancelling one means "drop it", for either.
 *
 * The strip renders nothing at all when the queue is empty: an empty box above
 * every composer would be noise, and there is no design surface for it.
 * A read failure is a muted line, not a thrown-away section — a queue that
 * cannot be read is worth saying out loud, because items may still be pending.
 */

import { cn } from "@/lib/cn";
import type { FollowupRecord } from "@/lib/api/followups";

export interface FollowupQueueProps {
  followups: readonly FollowupRecord[];
  /** The read failed; the queue may still hold items. */
  error?: Error | null;
  /** The row a cancel is in flight for. */
  cancellingId?: number | null;
  onCancel: (followupId: number) => void;
}

/** `follow-up` / `steering leftover` — the row's kind, in the design's pill. */
function kindLabel(kind: FollowupRecord["kind"]): string {
  return kind === "followup" ? "follow-up" : "steering leftover";
}

export function FollowupQueue({
  followups,
  error = null,
  cancellingId = null,
  onCancel,
}: FollowupQueueProps) {
  if (error !== null) {
    return (
      <p className="mb-[9px] font-mono text-2xs-plus text-faint">
        Could not read this lane&apos;s follow-up queue: {error.message}
      </p>
    );
  }

  if (followups.length === 0) return null;

  return (
    <section
      aria-label="Pending follow-ups"
      className="mb-[9px] flex flex-col gap-[5px]"
    >
      {followups.map((followup) => (
        <div key={followup.id} className="flex items-center gap-[8px]">
          <span className="shrink-0 rounded-sm bg-amber-tint px-[7px] py-[2px] font-mono text-2xs tracking-label text-amber-ink uppercase">
            {kindLabel(followup.kind)}
          </span>
          <span className="min-w-0 flex-1 truncate text-base text-secondary">
            {followup.content}
          </span>
          <button
            type="button"
            disabled={cancellingId === followup.id}
            onClick={() => onCancel(followup.id)}
            className={cn(
              "shrink-0 cursor-pointer border-none bg-transparent p-0 text-sm text-muted-fg",
              "transition-colors duration-[120ms] hover:text-ink",
              "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
              "disabled:pointer-events-none disabled:opacity-55",
            )}
          >
            {cancellingId === followup.id ? "cancelling…" : "cancel"}
          </button>
        </div>
      ))}
    </section>
  );
}
