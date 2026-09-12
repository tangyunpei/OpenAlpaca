/**
 * `Tab` (DESIGN_SPEC §3.36) — two sizes, one mechanism.
 *
 * `margin-bottom:-1px` pulls the 2px active underline over the container's 1px
 * bottom border so the two sit flush; keep it on every tab, active or not, or
 * the strip shifts by a pixel when the selection moves.
 *
 * `leading-[normal]` rides with each size, not in `base`: a font size overrides
 * `leading-*` in tailwind-merge, so a shared leading would be dropped.
 *
 * The design has no ARIA on tabs (§8.8); `role="tab"` + `aria-selected` are
 * added here, so the strip that holds them must carry `role="tablist"`.
 */

import { tv } from "@/lib/tv";

import { cn } from "@/lib/cn";

const tab = tv({
  base: cn(
    // `border-0 border-b-2`, never `border-none`: `border-none` sets
    // border-style, which would erase the underline the width then asks for.
    "-mb-px cursor-pointer border-0 border-b-2 bg-transparent font-sans font-medium",
    "transition-[color,border-color] duration-[120ms]",
    "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
  ),
  variants: {
    size: {
      /** File-panel tabs. */
      panel: "px-[10px] py-[6px] text-sm-plus leading-[normal]",
      /** Library detail tabs. */
      library: "px-[13px] py-[8px] text-base-plus leading-[normal]",
    },
    active: {
      true: "border-b-ink text-ink",
      false: "border-b-transparent text-muted-fg",
    },
  },
  defaultVariants: { size: "panel", active: false },
});

export type TabSize = "panel" | "library";

export interface TabProps extends Omit<
  React.ButtonHTMLAttributes<HTMLButtonElement>,
  "children"
> {
  label: string;
  active: boolean;
  size?: TabSize;
}

export function Tab({
  label,
  active,
  size = "panel",
  className,
  ...rest
}: TabProps) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      className={cn(tab({ size, active }), className)}
      {...rest}
    >
      {label}
    </button>
  );
}
