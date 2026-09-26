/**
 * Which pending confirmation the composer answers, in words.
 *
 * The composer's Approve, Deny and Always allow, the Enter binding and the
 * palette's Approve row all answer the **oldest** pending prompt — the order
 * the daemon raised them in — while the transcript scrolls to the newest card,
 * and two calls of one tool read alike on a line that names only the tool. A
 * lead running subagents in parallel raises several at once. So when more
 * than one is pending, the card the composer answers says so and is outlined
 * in red (like the composer's own answer box), and the bar and the palette's
 * row say they answer the oldest. The chat connectors tell a queued prompt
 * its place the same way (`(2 of 2 pending; each /yes or /no answers the
 * oldest first)`).
 *
 * Display only: what is answered is unchanged — the oldest prompt's request
 * id.
 */

/** `oldest of 3 pending`, or `null` when one prompt or none is pending. */
export function oldestOfPending(total: number): string | null {
  return total > 1 ? `oldest of ${total} pending` : null;
}
