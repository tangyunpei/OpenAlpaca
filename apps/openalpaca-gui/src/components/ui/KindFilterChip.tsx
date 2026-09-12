/**
 * `KindFilterChip` (DESIGN_SPEC §3.29) — the Library's kind filter bar.
 *
 * Sans 11px, not the mono 10px model chip (§3.35's chip row); the two are
 * different controls that happen to share a pill silhouette.
 *
 * The label→kind mapping is part of the spec: Media covers two artifact kinds,
 * and All covers everything, so the filter is a predicate, not an equality.
 */

import { tv } from "@/lib/tv";

import { cn } from "@/lib/cn";

import type { FileKind } from "./file-kind";

const chip = tv({
  base: cn(
    "cursor-pointer rounded-pill px-[9px] py-[4px] font-sans text-sm leading-[normal]",
    "transition-[background-color,border-color,color] duration-[120ms]",
    "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
  ),
  variants: {
    selected: {
      true: "border border-ink bg-ink text-on-dark",
      false: "border border-line bg-transparent text-secondary",
    },
  },
  defaultVariants: { selected: false },
});

export const KIND_FILTERS = [
  "All",
  "Docs",
  "Code",
  "Output",
  "Data",
  "Media",
  "Plans",
] as const;

export type KindFilter = (typeof KIND_FILTERS)[number];

/** Which file kinds a filter admits. `All` admits every kind. */
const FILTER_KINDS: Record<Exclude<KindFilter, "All">, readonly FileKind[]> = {
  Docs: ["md"],
  Code: ["code"],
  Output: ["term"],
  Data: ["table"],
  Media: ["image", "html"],
  Plans: ["plan"],
};

export function matchesKindFilter(filter: string, kind: FileKind): boolean {
  if (filter === "All") return true;
  const kinds = FILTER_KINDS[filter as Exclude<KindFilter, "All">];
  return kinds !== undefined && kinds.includes(kind);
}

export interface KindFilterChipProps {
  label: string;
  selected: boolean;
  onSelect: (label: string) => void;
  className?: string;
}

export function KindFilterChip({
  label,
  selected,
  onSelect,
  className,
}: KindFilterChipProps) {
  return (
    <button
      type="button"
      aria-pressed={selected}
      onClick={() => onSelect(label)}
      className={cn(chip({ selected }), className)}
    >
      {label}
    </button>
  );
}
