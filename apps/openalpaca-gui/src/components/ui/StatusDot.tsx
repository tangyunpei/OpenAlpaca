/**
 * `StatusDot` (DESIGN_SPEC §3.20).
 *
 * Two sizes: 6px in the rail (1px border) and 7px in run cards and work-list
 * rows (1.5px border). Filled states carry no border at all; the two outlined
 * states are drawn as a ring on a transparent fill.
 *
 * The design conveys the running state with colour and motion only (§8.8), so
 * the dot is exposed to assistive tech as an image with the status word, and
 * only hidden when a `StatusLabel` sitting next to it already says it.
 */

import { tv } from "@/lib/tv";

import { cn } from "@/lib/cn";

import { statusAria, statusPulses, type UiStatus } from "./status";

const dot = tv({
  base: "block shrink-0 rounded-full",
  variants: {
    status: {
      // Filled states set the border to the fill colour rather than removing
      // it: the box is border-box, so a same-colour ring is invisible, and it
      // keeps the size variant's border width from resolving to currentColor.
      running: "border-green bg-green",
      queued: "border-gold bg-transparent",
      paused: "border-blue bg-blue",
      done: "border-disabled bg-disabled",
      cancelled: "border-muted-fg bg-transparent",
      failed: "border-red bg-transparent",
    },
    size: {
      6: "h-[6px] w-[6px] border-[1px]",
      7: "h-[7px] w-[7px] border-[1.5px]",
    },
    pulse: {
      true: "animate-pulse-oa",
      false: "",
    },
  },
  defaultVariants: { size: 7, pulse: false },
});

export interface StatusDotProps {
  status: UiStatus;
  /** 6 in the rail, 7 in cards and list rows. */
  size?: 6 | 7;
  /** Set when an adjacent `StatusLabel` already names the status. */
  decorative?: boolean;
  className?: string;
}

export function StatusDot({
  status,
  size = 7,
  decorative = false,
  className,
}: StatusDotProps) {
  const classes = cn(
    dot({ status, size, pulse: statusPulses(status) }),
    className,
  );

  if (decorative) return <span aria-hidden className={classes} />;
  return (
    <span role="img" aria-label={statusAria(status)} className={classes} />
  );
}
