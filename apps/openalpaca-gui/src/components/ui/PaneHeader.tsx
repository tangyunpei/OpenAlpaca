/**
 * `PaneHeader` (DESIGN_SPEC §3.7) — the 46px strip at the top of every pane.
 *
 * Three flavours, differing only in horizontal padding and title size:
 *   `chat`  — 26px padding, 14.5px title, a mono 10.5px date on the baseline
 *   `pane`  — 18px padding, 14.5px title, mono 10px counts (Work, Library)
 *   `aside` — 18px padding, 13.5px title (the chat aside's Work header)
 */

import { tv } from "@/lib/tv";

import { cn } from "@/lib/cn";

const header = tv({
  base: "flex h-[46px] shrink-0 items-center justify-between border-b border-line-subtle",
  variants: {
    variant: {
      chat: "px-[26px]",
      pane: "px-[18px]",
      aside: "px-[18px]",
    },
  },
  defaultVariants: { variant: "pane" },
});

export type PaneHeaderVariant = "chat" | "pane" | "aside";

export interface PaneHeaderProps {
  title: string;
  /** The mono meta beside the title — a date in chat, counts elsewhere. */
  meta?: React.ReactNode;
  variant?: PaneHeaderVariant;
  /** Right-hand controls. */
  children?: React.ReactNode;
  className?: string;
}

export function PaneHeader({
  title,
  meta,
  variant = "pane",
  children,
  className,
}: PaneHeaderProps) {
  return (
    <header className={cn(header({ variant }), className)}>
      <div className="flex items-baseline gap-[10px]">
        <h1
          className={cn(
            "m-0 font-semibold text-ink",
            variant === "aside"
              ? "text-md-plus"
              : "text-lg-plus tracking-tight",
          )}
        >
          {title}
        </h1>
        {meta !== undefined && (
          <span
            className={cn(
              "font-mono text-muted-fg",
              variant === "chat" ? "text-xs-plus" : "text-xs",
            )}
          >
            {meta}
          </span>
        )}
      </div>
      {children !== undefined && (
        <div className="flex items-center gap-[6px]">{children}</div>
      )}
    </header>
  );
}
