/**
 * `LogTag` (DESIGN_SPEC §3.28).
 *
 * The fixed 58px width is load-bearing: it is what keeps the message column of
 * an event log aligned down the card. Anything the daemon emits that is not one
 * of the four known tags falls back to the neutral `run` styling.
 */

import { tv } from "@/lib/tv";

import { cn } from "@/lib/cn";

const tag = tv({
  base: "w-[58px] shrink-0 rounded-sm px-[6px] py-[2px] text-center font-mono text-2xs tracking-tag uppercase",
  variants: {
    tone: {
      tool: "bg-amber-tint text-amber-ink",
      steer: "bg-blue-tint text-blue",
      artifact: "bg-violet-tint text-violet",
      spawn: "bg-green-tint text-green",
      run: "bg-muted text-tertiary",
    },
  },
  defaultVariants: { tone: "run" },
});

/** The tags the design draws. */
export type LogTagTone = "tool" | "steer" | "artifact" | "spawn" | "run";

const TONES: readonly string[] = ["tool", "steer", "artifact", "spawn", "run"];

/** Anything unknown renders as `run` — §3.28's "anything else" row. */
export function toLogTone(value: string): LogTagTone {
  const lower = value.toLowerCase();
  return TONES.includes(lower) ? (lower as LogTagTone) : "run";
}

export interface LogTagProps {
  /** A `RunEventTag`, or any raw event name — unknown values degrade to `run`. */
  value: string;
  className?: string;
}

export function LogTag({ value, className }: LogTagProps) {
  const tone = toLogTone(value);
  return <span className={cn(tag({ tone }), className)}>{value}</span>;
}
