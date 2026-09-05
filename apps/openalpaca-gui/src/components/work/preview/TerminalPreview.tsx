/**
 * Terminal / tool-output renderer (DESIGN_SPEC §3.25c) — the one dark card.
 *
 * The header dot is green on exit 0 and red otherwise. When the exit code is
 * unknown — which is every artifact today, because nothing records per-kind
 * metadata (GAP-05) — the dot is neutral and the `exit …` text is simply
 * absent rather than assumed to be zero.
 */

import { cn } from "@/lib/cn";

import type { PreviewSize, TerminalLine } from "./types";

export interface TerminalPreviewProps {
  lines: readonly TerminalLine[];
  size: PreviewSize;
  /** `null` when unknown — do not default it to 0. */
  exitCode?: number | null;
  /** `1.4s`. */
  duration?: string | null;
  /** Shown when there is no exit code to show. */
  label?: string | null;
  className?: string;
}

export function TerminalPreview({
  lines,
  size,
  exitCode,
  duration,
  label,
  className,
}: TerminalPreviewProps) {
  const full = size === "full";
  const known = exitCode !== null && exitCode !== undefined;
  const parts: string[] = [];
  if (known) parts.push(`exit ${exitCode}`);
  else if (label !== null && label !== undefined && label !== "")
    parts.push(label);
  if (duration !== null && duration !== undefined && duration !== "")
    parts.push(duration);

  return (
    <div
      className={cn(
        "overflow-hidden border border-line bg-terminal",
        full ? "rounded-3xl" : "rounded-2xl",
        className,
      )}
    >
      <div
        className={cn(
          "flex items-center border-b border-terminal-line font-mono text-term-head",
          full
            ? "gap-[8px] px-[14px] py-[9px] text-xs"
            : "gap-[7px] px-[11px] py-[8px] text-2xs-plus",
        )}
      >
        <span
          aria-hidden
          className={cn(
            "h-[6px] w-[6px] shrink-0 rounded-full",
            !known ? "bg-term-head" : exitCode === 0 ? "bg-green" : "bg-red",
          )}
        />
        <span className="min-w-0 flex-1 truncate">{parts.join(" · ")}</span>
      </div>

      <pre
        className={cn(
          "m-0 overflow-x-auto font-mono leading-[1.8] text-term-fg",
          full ? "p-[14px] text-sm-plus" : "p-[11px] text-xs-plus",
        )}
      >
        {lines.map((line, index) => (
          <div
            // Output lines have no id; their order is their identity.
            key={index}
            className={line.prompt ? "text-term-prompt" : undefined}
          >
            {line.text === "" ? " " : line.text}
          </div>
        ))}
      </pre>
    </div>
  );
}
