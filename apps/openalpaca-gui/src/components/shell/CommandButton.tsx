/**
 * `CommandButton` (DESIGN_SPEC §3.5) — the only visible way into the command
 * palette besides ⌘K.
 */

import { cn } from "@/lib/cn";

export interface CommandButtonProps {
  onOpen: () => void;
  className?: string;
}

export function CommandButton({ onOpen, className }: CommandButtonProps) {
  return (
    <button
      type="button"
      onClick={onOpen}
      className={cn(
        "flex w-full cursor-pointer items-center gap-[8px] rounded-lg border border-line-popover bg-cmd px-[10px] py-[7px] font-mono text-xs-plus leading-[normal] text-secondary",
        "transition-[background-color] duration-[120ms] hover:bg-cmd-hover",
        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue",
        className,
      )}
    >
      <span className="font-semibold">⌘K</span>
      <span className="flex-1 text-left">Command</span>
    </button>
  );
}
