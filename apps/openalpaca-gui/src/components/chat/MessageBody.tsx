/**
 * The body of a message row (DESIGN_SPEC §3.10).
 *
 * `text-wrap: pretty` is on every long body in the design (§8.10) and is kept.
 * Single newlines inside a paragraph are preserved with `whitespace-pre-line`
 * so a model's line breaks survive; blank lines become new paragraphs.
 *
 * A completion report is written in markdown, so the transcript renders the
 * part of it a chat row can honestly show: emphasis and lists, beside the
 * inline code it already did (G8). Elements, never HTML — `parseProse` hands
 * back structure and this maps it, so nothing here can inject anything.
 */

import { cn } from "@/lib/cn";

import { parseProse, type ProseSegment } from "./prose";

export interface MessageBodyProps {
  text: string;
  /** Assistant bodies carry a 14px tail; user bodies do not. */
  spacing?: "user" | "assistant";
  /** Rendered at the end of the last block — the streaming caret. */
  trailing?: React.ReactNode;
  className?: string;
}

/** One run of text: code, bold, italic, or plain. */
function Segment({ segment }: { segment: ProseSegment }) {
  if (segment.code) {
    return (
      <code className="rounded-xs bg-code-chip px-[5px] py-px font-mono text-md">
        {segment.text}
      </code>
    );
  }
  if (segment.strong === true) {
    return <strong className="font-semibold text-ink">{segment.text}</strong>;
  }
  if (segment.em === true) return <em>{segment.text}</em>;
  return <span>{segment.text}</span>;
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
      {blocks.map((block, index) => {
        const tail =
          (spacing === "assistant" && index === last) || index < last
            ? "mb-[14px]"
            : undefined;

        if (block.kind === "list") {
          const List = block.ordered ? "ol" : "ul";
          return (
            <List
              key={block.key}
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
                  {index === last && item === block.items.length - 1
                    ? trailing
                    : null}
                </li>
              ))}
            </List>
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
            {index === last && trailing}
          </p>
        );
      })}
    </>
  );
}
