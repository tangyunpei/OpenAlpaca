/**
 * `NavItem` (DESIGN_SPEC §3.3) — the rail's Chat / Work / Library / Settings
 * buttons, and the single most reused control in the design.
 *
 * Active is an ink fill with paper text; inactive is transparent. §3.3 notes
 * that the design declares **no** hover state and that `#DBD6CB` would match
 * the family — that addition is taken here, since a rail item that does not
 * respond to the pointer reads as disabled.
 *
 * Two trailing treatments: Work carries a red `CountBadge`, Library a muted
 * mono count.
 */

import { cn } from "@/lib/cn";
import { CountBadge } from "@/components/ui";

export interface NavItemProps {
  icon: React.ReactNode;
  label: string;
  active: boolean;
  onSelect: () => void;
  /** Red badge (Work) or muted mono count (Library); omit for none. */
  count?: number;
  countStyle?: "badge" | "muted";
}

export function NavItem({
  icon,
  label,
  active,
  onSelect,
  count,
  countStyle = "muted",
}: NavItemProps) {
  return (
    <button
      type="button"
      aria-current={active ? "page" : undefined}
      onClick={onSelect}
      className={cn(
        "flex cursor-pointer items-center gap-[9px] rounded-lg border-none px-[10px] py-[8px] text-left font-sans text-md leading-[normal] font-medium",
        "transition-[background-color,color] duration-[120ms]",
        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
        active
          ? "bg-ink text-on-dark"
          : "bg-transparent text-secondary hover:bg-cmd",
      )}
    >
      {icon}
      <span className="flex-1">{label}</span>
      {count !== undefined &&
        (countStyle === "badge" ? (
          <CountBadge count={count} />
        ) : (
          <span className="font-mono text-xs opacity-60">{count}</span>
        ))}
    </button>
  );
}
