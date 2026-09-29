/**
 * "Continuing from: <topic> (ended <date>)" — the muted line above the
 * transcript while the latest turn was given the previous conversation on
 * this lane and project (design 11 §4 (b), §5; owner decisions CQ-5, CQ-7).
 *
 * It comes from one datum, `done.continued_from`: a new conversation is given
 * its predecessor's working state for its first three user turns, and the
 * line says so as long as it is. The topic is the summariser's text, drawn
 * through `EscapedText`; a predecessor that left no state is "the previous
 * conversation". The date is the predecessor's `ended_at`, when the window's
 * conversation list holds it.
 */

import { formatDayMonth } from "@/components/chat";
import { EscapedText } from "@/components/ui";
import type { ContinuedFrom } from "@/lib/chat-stream";
import { escapeHiddenLine } from "@/lib/hidden-chars";

export interface ContinuingFromLineProps {
  continuedFrom: ContinuedFrom;
  /** The previous conversation's `ended_at`, or `null` when unknown. */
  endedAt: string | null;
}

export function ContinuingFromLine({
  continuedFrom,
  endedAt,
}: ContinuingFromLineProps) {
  const { topic } = continuedFrom;
  return (
    <p className="mb-[20px] font-mono text-2xs-plus text-faint">
      Continuing from:{" "}
      {topic === "" ? (
        "the previous conversation"
      ) : (
        <EscapedText escaped={escapeHiddenLine(topic)} />
      )}
      {endedAt !== null && ` (ended ${formatDayMonth(endedAt)})`}
    </p>
  );
}
