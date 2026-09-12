/**
 * `FileBadge` (DESIGN_SPEC §3.22) — a coloured rounded square holding a 2–3
 * letter mono abbreviation. Five sizes, each with its own font size, and a
 * different radius at the extremes (14px → 3, 32px → 7, everything else → 4).
 *
 * `leading-none` sits with each size rather than in `base`: a font size
 * overrides `leading-*` in tailwind-merge, so a shared leading would be
 * dropped by the size variant.
 */

import { tv } from "@/lib/tv";

import { cn } from "@/lib/cn";

import { fileAbbr, type FileKind } from "./file-kind";

const badge = tv({
  base: "flex shrink-0 items-center justify-center font-mono font-semibold text-[#fff]",
  variants: {
    kind: {
      md: "bg-badge-md",
      code: "bg-badge-rs",
      plan: "bg-badge-pln",
      term: "bg-badge-out",
      table: "bg-badge-csv",
      html: "bg-badge-web",
      image: "bg-badge-img",
    },
    size: {
      14: "h-[14px] w-[14px] rounded-xs text-[6.5px] leading-none",
      16: "h-[16px] w-[16px] rounded-sm text-[6.5px] leading-none",
      17: "h-[17px] w-[17px] rounded-sm text-[7px] leading-none",
      19: "h-[19px] w-[19px] rounded-sm text-[8px] leading-none",
      32: "h-[32px] w-[32px] rounded-lg text-[9.5px] leading-none",
    },
  },
  defaultVariants: { size: 17 },
});

export type FileBadgeSize = 14 | 16 | 17 | 19 | 32;

export interface FileBadgeProps {
  kind: FileKind;
  size?: FileBadgeSize;
  /** Extension or highlight.js id; only `code` varies by it. */
  language?: string | null;
  /** Override the derived text outright. */
  abbr?: string;
  className?: string;
}

export function FileBadge({
  kind,
  size = 17,
  language,
  abbr,
  className,
}: FileBadgeProps) {
  return (
    <span aria-hidden className={cn(badge({ kind, size }), className)}>
      {abbr ?? fileAbbr(kind, language)}
    </span>
  );
}
