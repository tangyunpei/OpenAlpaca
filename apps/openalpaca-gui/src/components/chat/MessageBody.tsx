/**
 * The `<p>` body of a message row (DESIGN_SPEC §3.10).
 *
 * `text-wrap: pretty` is on every long body in the design (§8.10) and is kept.
 * Single newlines inside a paragraph are preserved with `whitespace-pre-line`
 * so a model's line breaks survive; blank lines become new paragraphs.
 */

import { cn } from "@/lib/cn";

import { parseProse } from "./prose";

export interface MessageBodyProps {
  text: string;
  /** Assistant bodies carry a 14px tail; user bodies do not. */
  spacing?: "user" | "assistant";
  /** Rendered at the end of the last paragraph — the streaming caret. */
  trailing?: React.ReactNode;
  className?: string;
}

export function MessageBody({
  text,
  spacing = "assistant",
  trailing,
  className,
}: MessageBodyProps) {
  const blocks = parseProse(text);
  const last = blocks.length - 1;

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
      {blocks.map((block, index) => (
        <p
          key={block.key}
          className={cn(
            "m-0 text-xl leading-[1.6] [text-wrap:pretty] whitespace-pre-line text-ink",
            spacing === "assistant" && index === last && "mb-[14px]",
            index < last && "mb-[14px]",
            className,
          )}
        >
          {block.segments.map((segment, segmentIndex) =>
            segment.code ? (
              <code
                key={segmentIndex}
                className="rounded-xs bg-code-chip px-[5px] py-px font-mono text-md"
              >
                {segment.text}
              </code>
            ) : (
              <span key={segmentIndex}>{segment.text}</span>
            ),
          )}
          {index === last && trailing}
        </p>
      ))}
    </>
  );
}
