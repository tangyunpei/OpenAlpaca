/**
 * `SectionCard` (DESIGN_SPEC §3.27) — the card that wraps Timeline, Output and
 * Event log in the work detail, and the settings cards in §3.32.
 *
 * Two header treatments:
 *   `framed` — a bordered header strip (Output, Event log), body flush to the
 *              card edges so rows can span the full width.
 *   `padded` — no strip; the eyebrow sits inside the card's own padding
 *              (Timeline, and the settings cards).
 *
 * `SectionEmpty` is the design's empty state. `note` is where an unbacked
 * surface names the API it is waiting on (API_MAP §3) — the empty copy stays
 * the design's, the note explains the absence instead of inventing rows.
 */

import { cn } from "@/lib/cn";

import { Eyebrow } from "./Eyebrow";

export type SectionCardVariant = "framed" | "padded";

export interface SectionCardProps {
  /** Rendered as the mono uppercase eyebrow. */
  title?: string;
  variant?: SectionCardVariant;
  /** Right-hand slot in a `framed` header strip. */
  action?: React.ReactNode;
  children?: React.ReactNode;
  className?: string;
}

export function SectionCard({
  title,
  variant = "framed",
  action,
  children,
  className,
}: SectionCardProps) {
  const padded = variant === "padded";
  return (
    <section
      className={cn(
        "mb-[16px] overflow-hidden rounded-3xl border border-line bg-raised",
        padded && "px-[18px] py-[16px]",
        className,
      )}
    >
      {title !== undefined &&
        (padded ? (
          <Eyebrow size={9.5} className="mb-[12px]">
            {title}
          </Eyebrow>
        ) : (
          <div className="flex items-center justify-between border-b border-line-hair px-[16px] py-[12px]">
            <Eyebrow size={9.5}>{title}</Eyebrow>
            {action}
          </div>
        ))}
      {children}
    </section>
  );
}

export interface SectionEmptyProps {
  /** The design's own empty-state sentence. */
  children: React.ReactNode;
  /** e.g. "Per-run event log not yet available". */
  note?: string;
  /** `false` inside a `padded` card, which already has padding. */
  padded?: boolean;
}

export function SectionEmpty({
  children,
  note,
  padded = true,
}: SectionEmptyProps) {
  return (
    <div className={cn(padded && "px-[16px] py-[14px]")}>
      <p className="m-0 text-md text-muted-fg">{children}</p>
      {note !== undefined && (
        <p className="mt-[6px] mb-0 font-mono text-2xs-plus text-faint">
          {note}
        </p>
      )}
    </div>
  );
}
