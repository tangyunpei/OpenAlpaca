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

import { useEffect, useLayoutEffect, useRef } from "react";

import { cn } from "@/lib/cn";

export interface ThinkingIndicatorProps {
  /**
   * Whether this turn has reasoning to show (S2). When it has, the label
   * becomes the disclosure control for the panel below the header.
   */
  hasReasoning?: boolean;
  /** Whether that panel is open — the label says which way it will go. */
  expanded?: boolean;
  onToggle?: () => void;
}

export function ThinkingIndicator({
  hasReasoning = false,
  expanded = false,
  onToggle,
}: ThinkingIndicatorProps) {
  return (
    <span className="flex items-center gap-[6px]" role="status">
      <span
        aria-hidden
        className="animate-pulse-oa block h-[6px] w-[6px] shrink-0 rounded-full bg-green"
      />
      {hasReasoning && onToggle !== undefined ? (
        <button
          type="button"
          onClick={onToggle}
          aria-expanded={expanded}
          className="cursor-pointer border-0 bg-transparent p-0 font-mono text-2xs-plus text-faint underline decoration-dotted underline-offset-2 hover:text-muted-fg focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
        >
          {expanded ? "thinking… (hide)" : "thinking… (show)"}
        </button>
      ) : (
        <span className="font-mono text-2xs-plus text-faint">thinking…</span>
      )}
    </span>
  );
}

export interface ReasoningPanelProps {
  /** The reasoning so far — already capped by `chat-stream`'s state. */
  text: string;
  className?: string;
}

/**
 * How near the bottom still counts as "at the bottom", in CSS pixels.
 *
 * Sub-pixel layout and a fractional line height mean an element scrolled all
 * the way down rarely reports exactly zero; a couple of pixels of slack is
 * what keeps a pinned box pinned.
 */
const PIN_SLACK = 4;

/** Whether `el` is scrolled to its bottom, within [`PIN_SLACK`]. */
export function isPinnedToBottom(el: {
  scrollHeight: number;
  scrollTop: number;
  clientHeight: number;
}): boolean {
  return el.scrollHeight - el.scrollTop - el.clientHeight <= PIN_SLACK;
}

/**
 * The model thinking out loud, under the assistant header (S2).
 *
 * Three constraints, and the CSS is all three: **muted** (the faint mono meta
 * type the header's own line uses, on the raised surface), **collapsible**
 * (the header's label is the disclosure), and **capped** — a fixed
 * `max-height` with its own scroll, so thirteen seconds of reasoning cannot
 * push the composer down the screen.
 *
 * The box **follows the newest text while it is pinned to the bottom** (V9).
 * A capped box with its own scrollbar shows its *oldest* lines once the
 * reasoning is longer than 88px, which is the least interesting part of it;
 * the comment here used to credit `overflow-anchor` for solving that, and
 * nothing set it (nor would it: scroll anchoring holds a position steady, it
 * does not chase an edge). So each new chunk scrolls the box down — unless the
 * reader has scrolled up, which un-pins it until they scroll back.
 */
export function ReasoningPanel({ text, className }: ReasoningPanelProps) {
  const box = useRef<HTMLDivElement>(null);
  /** Optimistic: a box that has never been scrolled is at its top *and* its
   *  bottom, and the reader has expressed no preference yet. */
  const pinned = useRef(true);

  // A native listener rather than `onScroll`: this fires on the element that
  // actually scrolls, whether that was a wheel, a drag or a keypress.
  useEffect(() => {
    const el = box.current;
    if (el === null) return;
    const onScroll = (): void => {
      pinned.current = isPinnedToBottom(el);
    };
    el.addEventListener("scroll", onScroll);
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  // Before paint, so the box never shows the old position for a frame.
  useLayoutEffect(() => {
    const el = box.current;
    if (el === null || !pinned.current) return;
    el.scrollTop = el.scrollHeight;
  }, [text]);

  return (
    <div
      ref={box}
      className={cn(
        "mb-[8px] max-h-[88px] overflow-y-auto rounded-md border border-line bg-raised px-[9px] py-[6px]",
        className,
      )}
    >
      <p className="m-0 font-mono text-2xs-plus leading-[1.5] whitespace-pre-wrap text-faint">
        {text}
      </p>
    </div>
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
