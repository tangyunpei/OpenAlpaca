/**
 * `StatusLabel` (DESIGN_SPEC §3.20) — always mono, weight 500, uppercase.
 * Three sizes: 10px in a run card, 9.5px in a list row, 10.5px in the work
 * detail header.
 */

import { tv } from "@/lib/tv";

import { cn } from "@/lib/cn";

import { STATUS_TEXT, type UiStatus } from "./status";

const label = tv({
  base: "font-mono font-medium",
  variants: {
    status: {
      running: "text-green",
      queued: "text-gold",
      paused: "text-blue",
      done: "text-tertiary",
      cancelled: "text-muted-fg",
      failed: "text-red",
    },
    size: {
      card: "text-xs",
      row: "text-2xs-plus",
      detail: "text-xs-plus",
    },
  },
  defaultVariants: { size: "card" },
});

export type StatusLabelSize = "card" | "row" | "detail";

/** Exposed so a test can assert that colour and size survive together. */
export function statusLabelClasses(
  status: UiStatus,
  size: StatusLabelSize = "card",
): string {
  return label({ status, size });
}

export interface StatusLabelProps {
  status: UiStatus;
  size?: StatusLabelSize;
  className?: string;
}

export function StatusLabel({
  status,
  size = "card",
  className,
}: StatusLabelProps) {
  return (
    <span className={cn(label({ status, size }), className)}>
      {STATUS_TEXT[status]}
    </span>
  );
}
