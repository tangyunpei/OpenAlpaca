/**
 * `DensityToggle` (DESIGN_SPEC §3.8) and `RunningNowPill` (§3.9).
 *
 * The toggle's label names the mode you would switch **to**, which is what the
 * design does; `aria-pressed` says which mode is currently on, because the
 * label alone reverses the usual reading.
 *
 * The pill is the design's own re-entry path for a fully collapsed aside —
 * rendered only when `!workOpen && !panelArt` (§4.2's `workClosed`).
 */

import { Button } from "@/components/ui";

export interface DensityToggleProps {
  dense: boolean;
  onToggle: () => void;
}

export function DensityToggle({ dense, onToggle }: DensityToggleProps) {
  return (
    <button
      type="button"
      aria-pressed={dense}
      onClick={onToggle}
      className="cursor-pointer rounded-md border border-line bg-transparent px-[9px] py-[4px] font-sans text-sm-plus leading-[normal] text-secondary transition-colors duration-[120ms] hover:bg-muted-hover focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
    >
      {dense ? "Comfortable" : "Compact"}
    </button>
  );
}

export interface RunningNowPillProps {
  count: number;
  onOpen: () => void;
}

export function RunningNowPill({ count, onOpen }: RunningNowPillProps) {
  return (
    <Button variant="secondaryXs" onClick={onOpen}>
      <span
        aria-hidden
        className="animate-pulse-oa block h-[6px] w-[6px] shrink-0 rounded-full bg-green"
      />
      {count} running
    </Button>
  );
}
