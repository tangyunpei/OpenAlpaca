/**
 * `StreamingIndicator` (DESIGN_SPEC §3.11) — **derived**, not in the design.
 *
 * The export has no thinking/typing component, so this is built entirely from
 * the design's own vocabulary: the pulsing 6px green dot it already uses for a
 * running run, the mono 9.5px faint meta type of the assistant header, and a
 * 2px ink caret. No spinner, no skeleton, no bouncing dots — nothing of the
 * kind exists in this language.
 *
 * Motion is decorative; the phase is announced as text for assistive tech
 * (§8.8 — the design's pulsing dot has no text equivalent).
 */

export function ThinkingIndicator() {
  return (
    <span className="flex items-center gap-[6px]" role="status">
      <span
        aria-hidden
        className="animate-pulse-oa block h-[6px] w-[6px] shrink-0 rounded-full bg-green"
      />
      <span className="font-mono text-2xs-plus text-faint">thinking…</span>
    </span>
  );
}

/** The caret appended to the partial body while deltas stream. */
export function StreamCaret() {
  return (
    <span
      aria-hidden
      className="animate-pulse-oa-fast ml-[2px] inline-block h-[1em] w-[2px] translate-y-[2px] bg-ink align-baseline"
    />
  );
}
