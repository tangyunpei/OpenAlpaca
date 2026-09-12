/**
 * `tailwind-variants`, taught this project's theme.
 *
 * Stock `tv` merges its own base/variant/slot classes with an unconfigured
 * `tailwind-merge`, which does not recognise the fractional type scale
 * (`text-base-plus`, `text-2xs-plus`, …) or the named shadows. It files them
 * under `text-color` / `shadow-color`, so a variant that sets a colour would
 * silently delete the size set by another variant — the 12.5px library tab
 * would render at the browser's default size.
 *
 * Every variant table in `components/ui` must import `tv` from here rather than
 * from `tailwind-variants` directly, so that the table's internal merge and
 * `cn`'s external merge share one view of the theme.
 */

import { createTV } from "tailwind-variants";

import { THEME_CLASS_GROUPS } from "./cn";

export const tv = createTV({
  twMergeConfig: { extend: { classGroups: THEME_CLASS_GROUPS } },
});

export type { VariantProps } from "tailwind-variants";
