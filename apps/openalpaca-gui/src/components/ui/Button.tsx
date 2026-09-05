/**
 * The button catalogue (DESIGN_SPEC §3.35) — all 17 rows, no extras.
 *
 * The export builds every button from an inline style string; §3.35 is the
 * transcription of those strings into a variant table, so this file is a
 * one-to-one translation of that table. Three deliberate departures, each
 * called out in the spec itself:
 *
 *  * `focus-visible` ring — the design has no focus styling at all (§3.35,
 *    §8.8). The spec prescribes `outline:2px solid #3A5FCC; offset 2px`.
 *  * a 120ms colour transition — permitted by §1.7 as "invisible at rest".
 *  * `disabled` dimming — nothing in the design is ever disabled; a disabled
 *    button still has to read as disabled.
 *
 * `leading-[normal]` is repeated on every row on purpose: a `text-*` class
 * carries this theme's paired line-height, and tailwind-merge treats a font
 * size as overriding `leading-*`, so a leading declared once in `base` would be
 * deleted by the variant's own size. The design declares no line-height on any
 * button, i.e. the browser default.
 *
 * Where §3.35 gives a *range* (`danger-ghost` at `5–6px 10–11px`, `pin` at two
 * sizes, `bare-link` at 9.5–11px) the smaller value is the variant default and
 * the larger call site overrides it via `className` — `cn` merges the conflict
 * correctly, which is the whole reason `cn` exists.
 */

import { tv, type VariantProps } from "@/lib/tv";

import { cn } from "@/lib/cn";

export const button = tv({
  base: cn(
    "inline-flex cursor-pointer items-center justify-center gap-[6px]",
    "font-sans",
    "transition-[background-color,border-color,color] duration-[120ms]",
    "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
    "disabled:pointer-events-none disabled:opacity-55",
  ),
  variants: {
    variant: {
      /** Approve — the one full-bleed primary. */
      primaryBlock:
        "rounded-lg border-none bg-ink p-[11px] text-md font-semibold text-on-dark hover:bg-ink-hover leading-[normal]",
      /** Send. */
      primaryMd:
        "rounded-lg border-none bg-ink px-[16px] py-[9px] text-md font-semibold text-on-dark hover:bg-ink-hover leading-[normal]",
      /** Open · Add provider. */
      primarySm:
        "rounded-md border-none bg-ink px-[11px] py-[5px] text-sm-plus font-medium text-on-dark hover:bg-ink-hover leading-[normal]",
      secondaryMd:
        "rounded-md border border-line bg-muted px-[11px] py-[6px] text-base font-medium text-ink hover:bg-muted-active leading-[normal]",
      secondarySm:
        "rounded-md border border-line bg-muted px-[10px] py-[5px] text-sm-plus font-medium text-ink hover:bg-muted-active leading-[normal]",
      /** The "N running" pill — weight 400, not 500. */
      secondaryXs:
        "rounded-md border border-line bg-muted px-[9px] py-[4px] text-sm-plus font-normal text-ink hover:bg-muted-active leading-[normal]",
      ghostSm:
        "rounded-md border border-line bg-transparent px-[10px] py-[5px] text-sm-plus font-normal text-secondary hover:bg-muted leading-[normal]",
      ghostXs:
        "rounded-[5px] border border-line bg-transparent px-[8px] py-[3px] text-sm font-normal text-secondary hover:bg-muted leading-[normal]",
      /** "Full view" — the only ghost that also darkens its label on hover. */
      ghost2xs:
        "rounded-[5px] border border-line bg-transparent px-[7px] py-[3px] text-sm font-normal text-tertiary hover:bg-muted-hover hover:text-ink leading-[normal]",
      /** Jump / Re-run inside a raised banner. */
      outlineRaised:
        "rounded-md border border-line bg-raised px-[11px] py-[6px] text-base font-medium text-ink hover:bg-muted leading-[normal]",
      /** Cancel. */
      dangerGhost:
        "rounded-md border border-amber-line bg-transparent px-[10px] py-[5px] text-sm-plus font-medium text-red-ink hover:bg-red-tint leading-[normal]",
      /** Bare textual link — "Library ↗", run links, "Always allow". */
      bareLink:
        "border-none bg-transparent p-0 text-2xs-plus font-normal text-muted-fg hover:text-ink leading-[normal]",
      /** Glyph-only control — `›`. */
      iconGlyph:
        "border-none bg-transparent px-[6px] py-[2px] text-xl font-normal text-muted-fg hover:text-ink leading-[normal]",
      pinOff:
        "rounded-[5px] border border-line bg-transparent px-[8px] py-[2px] text-xs-plus font-normal text-secondary leading-[normal]",
      pinOn:
        "rounded-[5px] border border-gold-line bg-gold-tint px-[8px] py-[2px] text-xs-plus font-normal text-gold-ink leading-[normal]",
      /** Model chips — mono. The sans 11px chip is `KindFilterChip` (§3.29). */
      chipOff:
        "rounded-pill border border-line bg-transparent px-[8px] py-[3px] font-mono text-xs font-normal text-secondary hover:border-line-hover leading-[normal]",
      chipOn:
        "rounded-pill border border-ink bg-ink px-[8px] py-[3px] font-mono text-xs font-normal text-on-dark leading-[normal]",
    },
  },
  defaultVariants: {
    variant: "secondarySm",
  },
});

export type ButtonVariant = NonNullable<VariantProps<typeof button>["variant"]>;

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
}

/** Always `type="button"` unless a caller opts into submit. */
export function Button({
  variant,
  className,
  type = "button",
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      className={cn(button({ variant }), className)}
      {...rest}
    />
  );
}

/**
 * `pin` and `chip` are boolean states in the design, not two unrelated
 * buttons; these keep call sites from re-deriving the pairing.
 */
export function pinVariant(pinned: boolean): ButtonVariant {
  return pinned ? "pinOn" : "pinOff";
}

export function chipVariant(selected: boolean): ButtonVariant {
  return selected ? "chipOn" : "chipOff";
}
