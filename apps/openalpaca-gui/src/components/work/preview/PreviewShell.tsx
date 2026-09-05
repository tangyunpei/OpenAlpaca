/**
 * The frame every artifact renderer shares (DESIGN_SPEC §3.25): a hairline
 * card that clips its content, one pixel rounder in the Library than in the
 * chat aside.
 */

import { cn } from "@/lib/cn";

import type { PreviewSize } from "./types";

export interface PreviewShellProps {
  size: PreviewSize;
  children: React.ReactNode;
  className?: string;
}

export function PreviewShell({ size, children, className }: PreviewShellProps) {
  return (
    <div
      className={cn(
        "overflow-hidden border border-line bg-raised",
        size === "full" ? "rounded-3xl" : "rounded-2xl",
        className,
      )}
    >
      {children}
    </div>
  );
}

/** The header strip shared by the code and diff renderers. */
export interface PreviewStripProps {
  size: PreviewSize;
  children: React.ReactNode;
  className?: string;
}

export function PreviewStrip({ size, children, className }: PreviewStripProps) {
  return (
    <div
      className={cn(
        "flex items-center border-b border-line-hair bg-sunken font-mono text-tertiary",
        size === "full"
          ? "gap-[10px] px-[14px] py-[9px] text-xs"
          : "gap-[8px] px-[11px] py-[8px] text-2xs-plus",
        className,
      )}
    >
      {children}
    </div>
  );
}

/** `+41` / `−6`, in that order, only when the counts are known. */
export function DiffCounters({
  added,
  removed,
  className,
}: {
  added: number | null | undefined;
  removed: number | null | undefined;
  className?: string;
}) {
  if (
    (added === null || added === undefined) &&
    (removed === null || removed === undefined)
  ) {
    return null;
  }
  return (
    <span className={cn("flex shrink-0 items-center gap-[8px]", className)}>
      {added !== null && added !== undefined && (
        <span className="text-green">{`+${added}`}</span>
      )}
      {removed !== null && removed !== undefined && (
        <span className="text-red">{`−${removed}`}</span>
      )}
    </span>
  );
}
