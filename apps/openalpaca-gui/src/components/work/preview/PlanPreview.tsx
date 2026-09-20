/**
 * Plan / checklist renderer (DESIGN_SPEC §3.25e).
 *
 * Three step states, each a 14/15px box: a filled green tick, a red ring for a
 * step waiting on a confirmation, and a neutral ring for one that has not
 * started. The blocked step's trailing mono note is the plan's own text (the
 * parser defaults it to "awaiting approval"), never a guess about *why* it is
 * blocked.
 */

import { cn } from "@/lib/cn";

import { planProgress } from "./parse";
import { PreviewShell } from "./PreviewShell";
import type { PlanStep, PreviewSize } from "./types";

export interface PlanPreviewProps {
  steps: readonly PlanStep[];
  size: PreviewSize;
  className?: string;
}

export function PlanPreview({ steps, size, className }: PlanPreviewProps) {
  const full = size === "full";

  return (
    <PreviewShell size={size} className={className}>
      <div
        className={cn(
          full ? "max-w-[620px] px-[20px] py-[18px]" : "px-[15px] py-[14px]",
        )}
      >
        <p
          className={cn(
            "m-0 font-mono tracking-eyebrow text-muted-fg uppercase",
            full ? "mb-[14px] text-xs" : "mb-[11px] text-2xs-plus",
          )}
        >
          {planProgress(steps)}
        </p>

        <ul
          className={cn(
            "m-0 flex list-none flex-col p-0",
            full ? "gap-[11px]" : "gap-[9px]",
          )}
        >
          {steps.map((step, index) => (
            <li
              // Steps are positional; two identical labels are legal in a plan.
              key={index}
              className={cn(
                "flex items-start",
                full ? "gap-[10px]" : "gap-[9px]",
              )}
            >
              <span
                aria-hidden
                className={cn(
                  "mt-[2px] flex shrink-0 items-center justify-center rounded-sm",
                  full
                    ? "h-[15px] w-[15px] text-[9px]"
                    : "h-[14px] w-[14px] text-[8px]",
                  step.state === "complete" && "bg-green text-[#fff]",
                  step.state === "blocked" &&
                    "border-[1.5px] border-red bg-transparent",
                  step.state === "pending" &&
                    "border-[1.5px] border-line-strong bg-transparent",
                )}
              >
                {step.state === "complete" ? "✓" : ""}
              </span>

              <span
                className={cn(
                  "flex-1 text-pretty",
                  full ? "text-md-plus" : "text-base-plus",
                  step.state === "complete" && "text-muted-fg line-through",
                  step.state === "blocked" && "font-medium text-ink",
                  step.state === "pending" && "text-tertiary",
                )}
              >
                {step.label}
                {step.state === "blocked" && step.note !== null && (
                  <span
                    className={cn(
                      "ml-[7px] font-mono font-normal text-amber-ink",
                      full ? "text-xs-plus" : "text-2xs-plus",
                    )}
                  >
                    {step.note}
                  </span>
                )}
              </span>
            </li>
          ))}
        </ul>
      </div>
    </PreviewShell>
  );
}
