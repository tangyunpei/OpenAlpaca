/**
 * `cn` — conditional class names, conflict-merged.
 *
 * The design system is expressed as Tailwind variant tables (DESIGN_SPEC §8.1),
 * so every primitive must let a caller override one property without the
 * variant's own class winning by source order. `tailwind-merge` does that, but
 * only if it can classify our class names — and the theme in `styles.css`
 * renames most of the scale:
 *
 *   `text-md-plus` is a **font size** (13.5px), not a text colour. Stock
 *   tailwind-merge only recognises t-shirt sizes for `text-*`, so it would file
 *   `text-md-plus` under `text-color` and let it delete `text-ink`.
 *   `shadow-card` has the same problem against `shadow-color`.
 *
 * Both groups are therefore extended with the theme's literal token names.
 * Purely additive tokens (`tracking-eyebrow`, `rounded-4xl`, colours) need no
 * entry: unknown classes are left alone, which is safe.
 */

import { clsx, type ClassValue } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

/** Every `--text-*` key in the `@theme` block (DESIGN_SPEC §1.9). */
const FONT_SIZES = [
  "2xs",
  "2xs-plus",
  "xs",
  "xs-plus",
  "sm",
  "sm-plus",
  "base",
  "base-plus",
  "md",
  "md-plus",
  "lg",
  "lg-plus",
  "xl",
  "2xl",
  "3xl",
  "4xl",
  "5xl",
] as const;

/** Every `--shadow-*` key in the `@theme` block. */
const SHADOWS = [
  "card",
  "card-active",
  "alert",
  "popover",
  "toast",
  "dialog",
] as const;

/**
 * Shared by `cn` and by the configured `tv` in `lib/tv.ts` — both mergers have
 * to classify the theme identically or a variant table and a `className`
 * override would disagree about what conflicts with what.
 */
export const THEME_CLASS_GROUPS = {
  "font-size": [{ text: [...FONT_SIZES] }],
  shadow: [{ shadow: [...SHADOWS] }],
};

const twMerge = extendTailwindMerge({
  extend: { classGroups: THEME_CLASS_GROUPS },
});

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
