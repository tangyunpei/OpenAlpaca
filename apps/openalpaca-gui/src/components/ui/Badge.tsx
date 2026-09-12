/**
 * Small non-interactive markers.
 *
 * `CountBadge` — the Work nav item's unread count (DESIGN_SPEC §3.3).
 * `Tag`        — the settings status tag (§3.32), four tones.
 *
 * `Tag`'s tone table is keyed by the *word* the daemon or the design uses
 * (`unwired`, `asks`, `live`, …) because that is how §3.32 specifies it;
 * anything else takes the neutral tone.
 */

import { tv } from "@/lib/tv";

import { cn } from "@/lib/cn";

export interface CountBadgeProps {
  count: number;
  className?: string;
}

/** `min-width:17px; height:17px` so a two-digit count widens rather than clips. */
export function CountBadge({ count, className }: CountBadgeProps) {
  return (
    <span
      className={cn(
        "flex h-[17px] min-w-[17px] items-center justify-center rounded-[9px] bg-red px-[4px] font-mono text-2xs-plus leading-none font-semibold text-[#fff]",
        className,
      )}
    >
      {count}
    </span>
  );
}

const tag = tv({
  base: "rounded-sm px-[6px] py-[2px] font-mono text-2xs tracking-label uppercase",
  variants: {
    tone: {
      warn: "bg-red-tint text-red-ink",
      asks: "bg-amber-tint text-amber-ink",
      live: "bg-green-tint text-green-ink",
      neutral: "bg-muted text-secondary",
    },
  },
  defaultVariants: { tone: "neutral" },
});

export type TagTone = "warn" | "asks" | "live" | "neutral";

/** §3.32's tone table: two words map to `warn`, two to `live`. */
export function toTagTone(value: string): TagTone {
  switch (value.toLowerCase()) {
    case "unwired":
    case "warn":
      return "warn";
    case "asks":
      return "asks";
    case "live":
    case "active":
      return "live";
    default:
      return "neutral";
  }
}

export interface TagProps {
  /** Rendered verbatim; the tone is derived from it unless `tone` is given. */
  value: string;
  tone?: TagTone;
  className?: string;
}

export function Tag({ value, tone, className }: TagProps) {
  return (
    <span className={cn(tag({ tone: tone ?? toTagTone(value) }), className)}>
      {value}
    </span>
  );
}
