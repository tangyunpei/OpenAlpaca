/**
 * Image and HTML renderers (DESIGN_SPEC §3.25f, §3.25g), plus the bar chart
 * §3.25g draws inside the HTML card.
 *
 * `ImagePreview` keeps the dashed box as the loading/missing state and swaps in
 * the real bytes when a `src` exists. It stays the *only* state today: the
 * content route is Bearer-authenticated, so an `<img src>` pointed at the
 * daemon is rejected (GAP-11) — a caller must fetch the bytes and hand over an
 * object URL.
 *
 * `HtmlPreview` sanitizes before rendering, for the same reason
 * `DocumentPreview` does: artifact bytes are agent output.
 */

import DOMPurify from "dompurify";
import { useMemo } from "react";

import { cn } from "@/lib/cn";

import { PreviewShell } from "./PreviewShell";
import type { ChartBar, PreviewSize } from "./types";

// ── Image (§3.25f) ──────────────────────────────────────────────────────────

export interface ImagePreviewProps {
  filename: string;
  size: PreviewSize;
  /** Object URL for the fetched bytes; `null` keeps the dashed placeholder. */
  src?: string | null;
  width?: number | null;
  height?: number | null;
  /** Muted line under the filename when the bytes are not loadable. */
  note?: string | null;
  className?: string;
}

export function ImagePreview({
  filename,
  size,
  src,
  width,
  height,
  note,
  className,
}: ImagePreviewProps) {
  const full = size === "full";
  const dimensions =
    width !== null &&
    width !== undefined &&
    height !== null &&
    height !== undefined
      ? `${width} × ${height}`
      : null;

  return (
    <PreviewShell
      size={size}
      className={cn(full ? "max-w-[700px] p-[14px]" : "p-[11px]", className)}
    >
      {src !== null && src !== undefined ? (
        <img src={src} alt={filename} className="block max-w-full rounded-md" />
      ) : (
        <div
          className={cn(
            "flex flex-col items-center justify-center rounded-md border border-dashed border-line-popover bg-muted",
            full ? "h-[340px] gap-[6px]" : "h-[220px] gap-[5px]",
          )}
        >
          <span
            className={cn(
              "font-mono text-muted-fg",
              full ? "text-sm" : "text-xs",
            )}
          >
            {filename}
          </span>
          {dimensions !== null && (
            <span
              className={cn(
                "font-mono text-faint",
                full ? "text-xs" : "text-2xs-plus",
              )}
            >
              {dimensions}
            </span>
          )}
          {note !== null && note !== undefined && (
            <span className="px-[16px] text-center font-mono text-2xs-plus text-faint">
              {note}
            </span>
          )}
        </div>
      )}
    </PreviewShell>
  );
}

// ── Bar chart (§3.25g) ──────────────────────────────────────────────────────

export interface BarChartProps {
  bars: readonly ChartBar[];
  size: PreviewSize;
  className?: string;
}

/** Heights are percentages of the row; the row itself is 70px / 96px. */
export function BarChart({ bars, size, className }: BarChartProps) {
  const full = size === "full";
  return (
    <div
      className={cn(
        "flex items-end",
        full ? "h-[96px] gap-[5px]" : "h-[70px] gap-[4px]",
        className,
      )}
    >
      {bars.map((bar, index) => (
        <span
          // Two bars may share a label (two months of the same name).
          key={index}
          title={bar.label}
          className={cn(
            "flex-1",
            full ? "rounded-t-[3px]" : "rounded-t-[2px]",
            bar.emphasis ? "bg-blue" : "bg-disabled",
          )}
          style={{ height: `${Math.min(100, Math.max(0, bar.height))}%` }}
        />
      ))}
    </div>
  );
}

// ── HTML (§3.25g) ───────────────────────────────────────────────────────────

export interface HtmlPreviewProps {
  filename: string;
  /** Raw HTML; sanitized here. */
  html: string;
  size: PreviewSize;
  className?: string;
}

export function HtmlPreview({
  filename,
  html,
  size,
  className,
}: HtmlPreviewProps) {
  const full = size === "full";
  const safe = useMemo(() => DOMPurify.sanitize(html), [html]);

  return (
    <PreviewShell size={size} className={className}>
      <div
        className={cn(
          "flex items-center border-b border-line-hair bg-sunken",
          full
            ? "gap-[8px] px-[12px] py-[8px]"
            : "gap-[7px] px-[10px] py-[7px]",
        )}
      >
        <span
          aria-hidden
          className={cn("flex", full ? "gap-[4px]" : "gap-[3px]")}
        >
          {[0, 1, 2].map((dot) => (
            <span
              key={dot}
              className={cn(
                "rounded-full bg-line-strong",
                full ? "h-[8px] w-[8px]" : "h-[7px] w-[7px]",
              )}
            />
          ))}
        </span>
        <span
          className={cn(
            "min-w-0 flex-1 truncate font-mono text-muted-fg",
            full ? "text-xs" : "text-2xs-plus",
          )}
        >
          {filename}
        </span>
      </div>

      <div
        className={cn(full ? "px-[30px] py-[26px]" : "px-[17px] py-[16px]")}
        // Sanitized immediately above.
        dangerouslySetInnerHTML={{ __html: safe }}
      />
    </PreviewShell>
  );
}
