/**
 * The tool confirmations the **daemon itself** has said are over (G1, Round G).
 *
 * A card can come back after it was answered: a `GET /v1/chat/confirmations`
 * read that left before the answer and lands after it still lists the prompt,
 * and the chat view's own record of what it has seen (`confirmationMeta`) is
 * component state that dies with the view and is cleared with the
 * conversation. This set is the memory that outlives both, so the snapshot
 * seed and every path that adopts a card into the stream skip a prompt the
 * daemon has already finished with.
 *
 * Only the daemon's own word goes in — never an inference:
 *
 *   * the answer route's `200` (it took this answer) or `404` (it holds no such
 *     prompt: answered already, timed out, withdrawn, or the daemon restarted);
 *   * the SSE `confirmation_resolved` frame;
 *   * the WebSocket `tool_confirmation_resolved` frame, heard while the chat
 *     view is open (`useChatStream` listens) — one resolved while chat is away
 *     is covered by the chat view's read on mount.
 *
 * A card this window retires by inference — its turn ended (R1), its run
 * finished, a snapshot no longer listed it — is **not** recorded. The
 * `{owner}:gui` lane is shared (T18): another client's main-loop prompt looks
 * exactly like this window's, and a later snapshot that still lists an id the
 * window only guessed was over must be able to draw it again.
 *
 * Request ids are `Uuid::new_v4` and never reused, so no id is ever taken back
 * out because it stopped being true. The set keeps the newest 1,000, oldest
 * dropped first, only so a window left open for weeks does not grow without
 * end: every read it guards is at most one poll (30 s) old, and no window
 * answers a thousand prompts in that time.
 */

const CAP = 1_000;

const over = new Set<string>();

/** The daemon said this prompt is over. */
export function noteConfirmationOver(requestId: string): void {
  over.add(requestId);
  if (over.size <= CAP) return;
  // A `Set` iterates in insertion order: the first entry is the oldest.
  const oldest = over.values().next();
  if (oldest.done !== true) over.delete(oldest.value);
}

/** Whether the daemon has said this prompt is over. */
export function isConfirmationOver(requestId: string): boolean {
  return over.has(requestId);
}

/**
 * Forget every recorded id. For tests only: the set outlives a test exactly
 * as it outlives a view, and the suites reuse request ids across tests.
 */
export function clearConfirmationsOver(): void {
  over.clear();
}
