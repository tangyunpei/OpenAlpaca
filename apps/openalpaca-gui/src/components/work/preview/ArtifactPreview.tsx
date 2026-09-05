/**
 * Kind → renderer (DESIGN_SPEC §3.25). One entry point for both sizes.
 *
 * The dispatcher owns the parse step so the renderers stay presentational, and
 * it owns the *no bytes yet* state — which is the normal state today, because
 * there is no artifact API to fetch bytes from (GAP-04). In that state it
 * renders the empty card plus the caller's note rather than a fake document.
 */

import { useMemo } from "react";

import { cn } from "@/lib/cn";

import { parseUnifiedDiff, sourceAsDiffLines, type DiffLine } from "../diff";

import { CodePreview } from "./CodePreview";
import { DocumentPreview } from "./DocumentPreview";
import { HtmlPreview, ImagePreview } from "./MediaPreview";
import { parsePlan, parseTable, parseTerminal } from "./parse";
import { PlanPreview } from "./PlanPreview";
import { PreviewShell } from "./PreviewShell";
import { TablePreview } from "./TablePreview";
import { TerminalPreview } from "./TerminalPreview";
import type { PreviewMeta, PreviewSize } from "./types";

/** A patch, or a plain file? The `@@` hunk marker is the only reliable tell. */
export function looksLikePatch(source: string): boolean {
  return (
    /^diff --git /m.test(source) ||
    /^@@ -\d+(,\d+)? \+\d+(,\d+)? @@/m.test(source)
  );
}

export function codeLines(source: string): DiffLine[] {
  return looksLikePatch(source)
    ? parseUnifiedDiff(source).lines
    : sourceAsDiffLines(source);
}

export interface PreviewUnavailableProps {
  size: PreviewSize;
  /** The design's own empty sentence. */
  children: React.ReactNode;
  /** The muted line naming the missing API. */
  note?: string | null;
  className?: string;
}

export function PreviewUnavailable({
  size,
  children,
  note,
  className,
}: PreviewUnavailableProps) {
  return (
    <PreviewShell size={size} className={className}>
      <div
        className={cn(
          size === "full" ? "px-[30px] py-[26px]" : "px-[17px] py-[16px]",
        )}
      >
        <p className="m-0 text-md text-muted-fg">{children}</p>
        {note !== null && note !== undefined && (
          <p className="mt-[6px] mb-0 font-mono text-2xs-plus text-faint">
            {note}
          </p>
        )}
      </div>
    </PreviewShell>
  );
}

export interface ArtifactPreviewProps {
  meta: PreviewMeta;
  /** Artifact bytes as text; `null` when they have not been (or cannot be) fetched. */
  content: string | null;
  size: PreviewSize;
  /** Object URL for image bytes — see GAP-11 for why it is not a daemon URL. */
  src?: string | null;
  /** Shown under the empty sentence when `content` is `null`. */
  note?: string | null;
  className?: string;
}

export function ArtifactPreview({
  meta,
  content,
  size,
  src,
  note,
  className,
}: ArtifactPreviewProps) {
  const parsed = useMemo(() => {
    if (content === null) return null;
    switch (meta.kind) {
      case "code":
        return { kind: "code" as const, lines: codeLines(content) };
      case "term":
        return { kind: "term" as const, lines: parseTerminal(content) };
      case "table":
        return { kind: "table" as const, table: parseTable(content) };
      case "plan":
        return { kind: "plan" as const, steps: parsePlan(content) };
      default:
        return { kind: "raw" as const };
    }
  }, [content, meta.kind]);

  // The image kind is the one renderer that draws without bytes: its dashed
  // placeholder *is* the missing state (§3.25f).
  if (meta.kind === "image") {
    return (
      <ImagePreview
        filename={meta.name}
        size={size}
        src={src ?? null}
        width={meta.width ?? null}
        height={meta.height ?? null}
        note={note ?? null}
        className={className}
      />
    );
  }

  if (content === null || parsed === null) {
    return (
      <PreviewUnavailable size={size} note={note} className={className}>
        Nothing to preview yet.
      </PreviewUnavailable>
    );
  }

  switch (parsed.kind) {
    case "code":
      return (
        <CodePreview
          path={meta.name}
          lines={parsed.lines}
          size={size}
          addedLines={meta.addedLines ?? null}
          removedLines={meta.removedLines ?? null}
          className={className}
        />
      );
    case "term":
      return (
        <TerminalPreview
          lines={parsed.lines}
          size={size}
          exitCode={meta.exitCode ?? null}
          duration={meta.duration ?? null}
          label={meta.name}
          className={className}
        />
      );
    case "table":
      return (
        <TablePreview table={parsed.table} size={size} className={className} />
      );
    case "plan":
      return (
        <PlanPreview steps={parsed.steps} size={size} className={className} />
      );
    case "raw":
      return meta.kind === "html" ? (
        <HtmlPreview
          filename={meta.name}
          html={content}
          size={size}
          className={className}
        />
      ) : (
        <DocumentPreview
          source={content}
          size={size}
          byline={meta.byline ?? null}
          className={className}
        />
      );
  }
}
