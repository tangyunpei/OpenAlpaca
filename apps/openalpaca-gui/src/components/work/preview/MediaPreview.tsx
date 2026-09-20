/**
 * Image and HTML renderers (DESIGN_SPEC §3.25f, §3.25g), plus the bar chart
 * §3.25g draws inside the HTML card.
 *
 * `ImagePreview` keeps the dashed box as the loading/missing state and swaps in
 * the real bytes when a `src` exists. That `src` is the daemon's own content
 * route with its `?token=` (`artifactContentUrl`), which the webview's CSP
 * allows for the loopback origins; an object URL works just as well. A `src`
 * the browser cannot load (410 gone, a rejected token, a CSP the webview
 * doesn't admit) falls back to that same dashed box via `onError` — a broken
 * `<img>` never renders on its own.
 *
 * `HtmlPreview` sanitizes before rendering, for the same reason
 * `DocumentPreview` does: artifact bytes are agent output.
 */

import DOMPurify from "dompurify";
import { useMemo, useState } from "react";

import { cn } from "@/lib/cn";

import { PreviewShell } from "./PreviewShell";
import type { ChartBar, PreviewSize } from "./types";

// ── Image (§3.25f) ──────────────────────────────────────────────────────────

export interface ImagePreviewProps {
  filename: string;
  size: PreviewSize;
  /**
   * The daemon's content route (`artifactContentUrl`), carrying its own
   * `?token=`; `null`/`undefined` keeps the dashed placeholder. A src that
   * fails to load (410 gone, a rejected token, a CSP the webview doesn't
   * admit) falls back to the same placeholder — see `onError` below.
   */
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
  // Tracks the specific `src` that failed so a later, different `src` (a new
  // artifact, or the same one after `missing` catches up and the caller
  // passes a fresh URL) gets its own attempt rather than staying broken.
  const [failedSrc, setFailedSrc] = useState<string | null>(null);
  const loadFailed = src !== null && src !== undefined && src === failedSrc;
  const hasSrc = src !== null && src !== undefined && !loadFailed;

  const dimensions =
    width !== null &&
    width !== undefined &&
    height !== null &&
    height !== undefined
      ? `${width} × ${height}`
      : null;

  // A load failure is the one thing this component learns for itself — the
  // browser rejected a URL the caller believed was good (gone, an expired
  // token, a CSP the webview doesn't admit) — so it overrides whatever note
  // the caller passed for the "no src at all" case.
  const placeholderNote = loadFailed
    ? "The image could not be loaded — Export or Reveal opens it in its own app."
    : note;

  return (
    <PreviewShell
      size={size}
      className={cn(full ? "max-w-[700px] p-[14px]" : "p-[11px]", className)}
    >
      {hasSrc ? (
        <img
          src={src}
          alt={filename}
          className="block max-w-full rounded-md"
          onError={() => setFailedSrc(src)}
        />
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
          {placeholderNote !== null && placeholderNote !== undefined && (
            <span className="px-[16px] text-center font-mono text-2xs-plus text-faint">
              {placeholderNote}
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
