/**
 * The mono uppercase eyebrow that labels every section in the design —
 * "Running now" (§3.4), "Parallel work" / "Output" / "Event log" (§3.27),
 * "Library · n files" (§3.23), the artifact-picker head.
 *
 * Two widths of letter-spacing appear: `.12em` (the common one) and `.1em`
 * (settings stat cells). Size is 9px except the 9.5px card labels.
 */

import { tv } from "@/lib/tv";

import { cn } from "@/lib/cn";

const eyebrow = tv({
  base: "font-mono uppercase",
  variants: {
    size: {
      9: "text-2xs",
      9.5: "text-2xs-plus",
    },
    tracking: {
      wide: "tracking-eyebrow-w",
      narrow: "tracking-eyebrow",
    },
    tone: {
      faint: "text-faint",
      muted: "text-muted-fg",
    },
  },
  defaultVariants: { size: 9, tracking: "wide", tone: "muted" },
});

export interface EyebrowProps {
  children: React.ReactNode;
  size?: 9 | 9.5;
  tracking?: "wide" | "narrow";
  tone?: "faint" | "muted";
  className?: string;
}

export function Eyebrow({
  children,
  size = 9,
  tracking = "wide",
  tone = "muted",
  className,
}: EyebrowProps) {
  return (
    <div className={cn(eyebrow({ size, tracking, tone }), className)}>
      {children}
    </div>
  );
}
