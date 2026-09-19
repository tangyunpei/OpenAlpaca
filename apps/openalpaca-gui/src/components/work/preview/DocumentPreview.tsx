/**
 * Document renderer (DESIGN_SPEC §3.25a) — the `md` kind.
 *
 * Markdown goes through `marked` and then DOMPurify before it reaches the DOM:
 * artifact bytes are agent output, i.e. untrusted, and the Tauri webview would
 * happily run a `<script>` in them. The element styling is the spec's table,
 * applied as descendant variants so one sanitized tree serves both sizes.
 */

import DOMPurify from "dompurify";
import { marked } from "marked";
import { useMemo } from "react";

import { cn } from "@/lib/cn";

import { PreviewShell } from "./PreviewShell";
import type { PreviewSize } from "./types";

/** `async: false` selects the synchronous overload, which returns a string. */
export function renderMarkdown(source: string): string {
  return DOMPurify.sanitize(marked.parse(source, { async: false }));
}

/**
 * A markdown table (P5).
 *
 * §3.25(a) does not list one — the spec's table renderer is (d), a *table
 * artifact* — so the look is extrapolated from (d): an uppercase mono header
 * on the sunken ground, hairline row rules, cells that breathe. Without any of
 * it the browser's bare `<table>` drew a 3-column table with no padding and no
 * rules at all, and in a 1080-point window's pane its cells read as one word
 * ("Fiber appearancehalo").
 *
 * `display:block` + `width:max-content` + `overflow-x:auto` is what makes it
 * *scroll* rather than squeeze: the table takes the width its content needs,
 * is capped at the pane, and scrolls inside itself — the same bargain the code
 * blocks already make, and the only one that keeps a wide table legible in a
 * pane the owner can narrow to a third of the window.
 */
const TABLE_PROSE = cn(
  "[&_table]:block [&_table]:w-max [&_table]:max-w-full [&_table]:overflow-x-auto [&_table]:border-collapse",
  "[&_th]:border-b [&_th]:border-line [&_th]:bg-sunken [&_th]:text-left [&_th]:font-mono [&_th]:font-normal [&_th]:text-muted-fg [&_th]:uppercase",
  "[&_td]:border-b [&_td]:border-line-subtle [&_td]:align-top [&_td]:text-body",
);

const COMPACT_PROSE = cn(
  "[&_p]:m-0 [&_p]:mb-[12px] [&_p]:text-base-plus [&_p]:leading-[1.65] [&_p]:text-body",
  "[&_h2]:m-0 [&_h2]:mb-[5px] [&_h2]:text-base-plus [&_h2]:font-semibold [&_h2]:text-ink",
  "[&_h3]:m-0 [&_h3]:mb-[5px] [&_h3]:text-base [&_h3]:font-semibold [&_h3]:text-ink",
  "[&_ul]:m-0 [&_ul]:mb-[12px] [&_ul]:list-disc [&_ul]:pl-[17px]",
  "[&_ol]:m-0 [&_ol]:mb-[13px] [&_ol]:list-decimal [&_ol]:pl-[17px]",
  "[&_li]:text-base-plus [&_li]:leading-[1.65] [&_li]:text-body",
  "[&_code]:rounded-xs [&_code]:bg-muted [&_code]:px-[4px] [&_code]:font-mono [&_code]:text-sm",
  "[&_pre]:m-0 [&_pre]:mb-[12px] [&_pre]:overflow-x-auto [&_pre]:rounded-md [&_pre]:bg-muted [&_pre]:p-[10px]",
  "[&_pre_code]:bg-transparent [&_pre_code]:px-0",
  "[&_blockquote]:m-0 [&_blockquote]:mb-[12px] [&_blockquote]:border-l-2 [&_blockquote]:border-gold [&_blockquote]:py-px [&_blockquote]:pl-[10px] [&_blockquote]:text-base [&_blockquote]:leading-[1.6] [&_blockquote]:text-tertiary",
  "[&_blockquote_p]:m-0 [&_blockquote_p]:text-base [&_blockquote_p]:leading-[1.6] [&_blockquote_p]:text-tertiary",
  TABLE_PROSE,
  "[&_table]:mb-[12px]",
  "[&_th]:px-[11px] [&_th]:py-[7px] [&_th]:text-2xs [&_th]:tracking-label",
  "[&_td]:px-[11px] [&_td]:py-[8px] [&_td]:text-sm-plus [&_td]:leading-[1.6]",
  "[&_a]:text-blue [&_a]:underline",
  "[&_*:last-child]:mb-0",
);

const FULL_PROSE = cn(
  "[&_p]:m-0 [&_p]:mb-[18px] [&_p]:text-md-plus [&_p]:leading-[1.7] [&_p]:text-body",
  "[&_h2]:m-0 [&_h2]:mb-[8px] [&_h2]:text-lg [&_h2]:font-semibold [&_h2]:text-ink",
  "[&_h3]:m-0 [&_h3]:mb-[8px] [&_h3]:text-md-plus [&_h3]:font-semibold [&_h3]:text-ink",
  "[&_ul]:m-0 [&_ul]:mb-[18px] [&_ul]:list-disc [&_ul]:pl-[20px]",
  "[&_ol]:m-0 [&_ol]:mb-[18px] [&_ol]:list-decimal [&_ol]:pl-[20px]",
  "[&_li]:text-md-plus [&_li]:leading-[1.7] [&_li]:text-body",
  "[&_code]:rounded-xs [&_code]:bg-muted [&_code]:px-[4px] [&_code]:py-px [&_code]:font-mono [&_code]:text-base",
  "[&_pre]:m-0 [&_pre]:mb-[18px] [&_pre]:overflow-x-auto [&_pre]:rounded-md [&_pre]:bg-muted [&_pre]:p-[12px]",
  "[&_pre_code]:bg-transparent [&_pre_code]:px-0",
  "[&_blockquote]:m-0 [&_blockquote]:mb-[18px] [&_blockquote]:border-l-2 [&_blockquote]:border-gold [&_blockquote]:py-[2px] [&_blockquote]:pl-[12px] [&_blockquote]:text-md [&_blockquote]:leading-[1.65] [&_blockquote]:text-tertiary",
  "[&_blockquote_p]:m-0 [&_blockquote_p]:text-md [&_blockquote_p]:leading-[1.65] [&_blockquote_p]:text-tertiary",
  TABLE_PROSE,
  "[&_table]:mb-[18px]",
  "[&_th]:px-[14px] [&_th]:py-[9px] [&_th]:text-2xs-plus [&_th]:tracking-label",
  "[&_td]:px-[14px] [&_td]:py-[10px] [&_td]:text-base-plus [&_td]:leading-[1.65]",
  "[&_a]:text-blue [&_a]:underline",
  "[&_*:last-child]:mb-0",
);

export interface DocumentPreviewProps {
  /** Raw markdown. */
  source: string;
  size: PreviewSize;
  /** Rendered above the body; omit to let the markdown supply its own `h1`. */
  title?: string | null;
  /** Mono line under the title — version, author, time. */
  byline?: string | null;
  className?: string;
}

export function DocumentPreview({
  source,
  size,
  title,
  byline,
  className,
}: DocumentPreviewProps) {
  const html = useMemo(() => renderMarkdown(source), [source]);
  const full = size === "full";

  return (
    <PreviewShell size={size} className={className}>
      <div
        className={cn(
          full ? "max-w-[660px] px-[30px] py-[26px]" : "px-[17px] py-[16px]",
        )}
      >
        {title !== null && title !== undefined && title !== "" && (
          <h1
            className={cn(
              "m-0 font-semibold text-pretty text-ink",
              full
                ? "mb-[4px] text-5xl tracking-tightest"
                : "mb-[3px] text-xl leading-[1.4] tracking-tighter",
            )}
          >
            {title}
          </h1>
        )}
        {byline !== null && byline !== undefined && byline !== "" && (
          <p
            className={cn(
              "m-0 font-mono text-muted-fg",
              full ? "mb-[20px] text-xs-plus" : "mb-[13px] text-2xs-plus",
            )}
          >
            {byline}
          </p>
        )}
        <div
          className={full ? FULL_PROSE : COMPACT_PROSE}
          // Sanitized immediately above; `marked` output is HTML by contract.
          dangerouslySetInnerHTML={{ __html: html }}
        />
      </div>
    </PreviewShell>
  );
}
