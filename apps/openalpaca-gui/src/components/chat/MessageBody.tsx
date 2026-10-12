/**
 * The body of a message row (DESIGN_SPEC §3.10).
 *
 * `text-wrap: pretty` is on every long body in the design (§8.10) and is kept.
 * Single newlines inside a paragraph are preserved with `whitespace-pre-line`
 * so a model's line breaks survive; blank lines become new paragraphs.
 *
 * A completion report — and a skill answer, and most of what a local model
 * writes — is markdown, so the transcript renders the part of it a chat row
 * can honestly show: emphasis and lists (G8), and since T3 fenced code,
 * headings, thematic breaks, tables and inline code. **Elements, never HTML**:
 * `parseProse` hands back structure and this maps it, so nothing here can
 * inject anything, and a markdown link or an `<img>` is simply text.
 *
 * The type scale is the transcript's own — 15px body, headings stepped above
 * it from the existing tokens — because a chat row is not a document and a
 * model's `#` is a section marker, not a title page.
 *
 * **`wrapAnywhere`** is for a text someone decides on — a candidate note the
 * owner confirms, whose escapes (`lib/hidden-chars`) are counted under it —
 * where nothing may sit out of sight: every block wraps a run with no break
 * opportunity anywhere, a code block wraps its lines instead of scrolling
 * sideways behind a scrollbar macOS does not draw, and a table is drawn as
 * its rows, one wrapping line each, its cells joined by ` · ` — a `<table>`
 * never shrinks a column below its cells' padding, so past about 26 columns
 * it is wider than the Note tab and its far cells scroll out of sight. Each
 * row keeps its cells past the header's width, where a transcript's table
 * drops them. A transcript keeps its own layout.
 */

import { cn } from "@/lib/cn";

import { parseProse, type HeadingLevel, type ProseSegment } from "./prose";

export interface MessageBodyProps {
  text: string;
  /** Assistant bodies carry a 14px tail; user bodies do not. */
  spacing?: "user" | "assistant";
  /** Rendered at the end of the last block — the streaming caret. */
  trailing?: React.ReactNode;
  className?: string;
  /**
   * Wrap every block, code included, and draw a table as its rows, one
   * wrapping line each, every cell kept (see above).
   */
  wrapAnywhere?: boolean;
}

/**
 * One run of text: code, bold, italic, or plain — and, since P4, a code span
 * an emphasis run wraps, which models write constantly (``**`file.md`**``).
 * The two are independent, so the emphasis element wraps the `<code>` rather
 * than the first match winning and the other being dropped.
 */
function Segment({ segment }: { segment: ProseSegment }) {
  const body = segment.code ? (
    <code className="rounded-xs bg-code-chip px-[5px] py-px font-mono text-md">
      {segment.text}
    </code>
  ) : (
    segment.text
  );
  if (segment.strong === true) {
    return <strong className="font-semibold text-ink">{body}</strong>;
  }
  if (segment.em === true) return <em>{body}</em>;
  return segment.code ? body : <span>{segment.text}</span>;
}

/** A table cell that holds no text: what `keepEveryCell` pads a row with. */
function isEmptyCell(cell: ProseSegment[]): boolean {
  return cell.every((segment) => segment.text === "");
}

/**
 * One row of a table someone decides on, as a line: its cells joined by
 * ` · `, its trailing empty cells (the header's padding among them) left
 * off, since they hold nothing to show.
 */
function TableRowLine({ row }: { row: ProseSegment[][] }) {
  let end = row.length;
  while (end > 0 && isEmptyCell(row[end - 1] ?? [])) end -= 1;
  return (
    <>
      {row.slice(0, end).map((cell, column) => (
        <span key={column}>
          {column > 0 && <span className="text-faint"> · </span>}
          <Segments segments={cell} />
        </span>
      ))}
    </>
  );
}

function Segments({ segments }: { segments: ProseSegment[] }) {
  return (
    <>
      {segments.map((segment, index) => (
        <Segment key={index} segment={segment} />
      ))}
    </>
  );
}

/**
 * Heading sizes, monotonically down to the 15px body without leaving the
 * scale: 19 · 17 · 15 · 14.5, all semibold, so h3 and h4 are told apart from
 * the paragraphs around them by weight rather than by growing.
 */
const HEADING_CLASS: Record<HeadingLevel, string> = {
  1: "text-3xl",
  2: "text-2xl",
  3: "text-xl",
  4: "text-lg-plus",
};

export function MessageBody({
  text,
  spacing = "assistant",
  trailing,
  className: extra,
  wrapAnywhere = false,
}: MessageBodyProps) {
  const blocks = parseProse(text, { keepEveryCell: wrapAnywhere });
  const last = blocks.length - 1;
  const className = cn(wrapAnywhere && "wrap-anywhere", extra);

  if (blocks.length === 0) {
    return trailing === undefined ? null : (
      <p
        className={cn(
          "m-0 text-xl leading-[1.6] [text-wrap:pretty] text-ink",
          className,
        )}
      >
        {trailing}
      </p>
    );
  }

  return (
    <>
      {blocks.map((block, index) => {
        const isLast = index === last;
        const tail =
          (spacing === "assistant" && isLast) || index < last
            ? "mb-[14px]"
            : undefined;

        if (block.kind === "list") {
          const List = block.ordered ? "ol" : "ul";
          return (
            <List
              key={block.key}
              // A resumed ordered list counts on from its own first marker
              // (T3); `start` is meaningless on a `<ul>` and omitted there.
              {...(block.ordered && block.start !== 1
                ? { start: block.start }
                : {})}
              className={cn(
                "m-0 pl-[22px] text-xl leading-[1.6] [text-wrap:pretty] text-ink",
                block.ordered ? "list-decimal" : "list-disc",
                tail,
                className,
              )}
            >
              {block.items.map((segments, item) => (
                <li key={item}>
                  <Segments segments={segments} />
                  {isLast && item === block.items.length - 1 ? trailing : null}
                </li>
              ))}
            </List>
          );
        }

        if (block.kind === "heading") {
          const Heading = `h${block.level}` as "h1" | "h2" | "h3" | "h4";
          return (
            <Heading
              key={block.key}
              className={cn(
                "m-0 font-semibold [text-wrap:pretty] text-ink",
                HEADING_CLASS[block.level],
                tail ?? "mb-[8px]",
                className,
              )}
            >
              <Segments segments={block.segments} />
              {isLast && trailing}
            </Heading>
          );
        }

        if (block.kind === "code") {
          return (
            <div key={block.key} className={cn("mb-[14px]", className)}>
              {block.language !== null && (
                <span className="mb-[4px] block font-mono text-2xs-plus tracking-label text-faint uppercase">
                  {block.language}
                </span>
              )}
              {/* Its own horizontal scroll: a long line is the block's
                  problem, never the transcript's. With `wrapAnywhere` the
                  lines wrap instead, so nothing scrolls out of sight. */}
              <pre
                className={cn(
                  "m-0 overflow-x-auto rounded-md border border-line-subtle bg-code-chip px-[11px] py-[9px] font-mono text-md leading-[1.5] text-ink",
                  wrapAnywhere && "wrap-anywhere whitespace-pre-wrap",
                )}
              >
                {block.text}
                {isLast && trailing}
              </pre>
            </div>
          );
        }

        if (block.kind === "quote") {
          return (
            <blockquote
              key={block.key}
              className={cn(
                "m-0 border-l-2 border-gold py-px pl-[11px] text-xl leading-[1.6] [text-wrap:pretty] whitespace-pre-line text-tertiary",
                tail,
                className,
              )}
            >
              <Segments segments={block.segments} />
              {isLast && trailing}
            </blockquote>
          );
        }

        if (block.kind === "rule") {
          return (
            <hr
              key={block.key}
              className={cn(
                "mt-0 mb-[14px] h-px border-0 bg-line-subtle",
                className,
              )}
            />
          );
        }

        if (block.kind === "table" && wrapAnywhere) {
          return (
            <div
              key={block.key}
              className={cn(
                "mb-[14px] text-base-plus leading-[1.5] text-ink",
                className,
              )}
            >
              {[block.header, ...block.rows].map((row, rowIndex) => (
                <p
                  key={rowIndex}
                  data-table-row=""
                  className={cn(
                    "m-0 py-[4px]",
                    rowIndex === 0
                      ? "border-b border-line font-semibold"
                      : "border-b border-line-subtle",
                  )}
                >
                  <TableRowLine row={row} />
                </p>
              ))}
              {isLast && trailing}
            </div>
          );
        }

        if (block.kind === "table") {
          return (
            <div
              key={block.key}
              // Wide tables scroll inside their own box, like the code blocks.
              className={cn("mb-[14px] overflow-x-auto", className)}
            >
              <table className="w-full border-collapse text-base-plus text-ink">
                <thead>
                  <tr>
                    {block.header.map((cell, column) => (
                      <th
                        key={column}
                        className="border-b border-line px-[9px] py-[5px] text-left font-semibold whitespace-nowrap"
                      >
                        <Segments segments={cell} />
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {block.rows.map((row, rowIndex) => (
                    <tr key={rowIndex}>
                      {row.map((cell, column) => (
                        <td
                          key={column}
                          className="border-b border-line-subtle px-[9px] py-[5px] align-top"
                        >
                          <Segments segments={cell} />
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
              {isLast && trailing}
            </div>
          );
        }

        return (
          <p
            key={block.key}
            className={cn(
              "m-0 text-xl leading-[1.6] [text-wrap:pretty] whitespace-pre-line text-ink",
              tail,
              className,
            )}
          >
            <Segments segments={block.segments} />
            {isLast && trailing}
          </p>
        );
      })}
    </>
  );
}
