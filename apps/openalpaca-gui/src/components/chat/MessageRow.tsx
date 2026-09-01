/**
 * Chat message rows (DESIGN_SPEC §3.10).
 *
 * There are **no avatars and no bubbles** anywhere in this design. A row is a
 * mono 9.5px uppercase speaker label over a 15px paragraph, at the same width
 * and alignment for both speakers; the only differentiation is the label's
 * colour. Do not add either.
 *
 * The wrapper's bottom margin is the density-controlled message gap (30 → 20,
 * §8.3), so it rides on the row rather than on a transcript-level `gap`: the
 * run-report card between two messages has its own 26px margin.
 */

import { cn } from "@/lib/cn";

import { assistantMetaLine, type AssistantMeta } from "./format";
import { MessageBody } from "./MessageBody";
import { StreamCaret, ThinkingIndicator } from "./StreamingIndicator";

/** `steer → connector audit` / `follow-up → connector audit`. */
export interface SteerRef {
  mode: "steer" | "queue";
  label: string;
}

export function messageGapClass(dense: boolean): string {
  return dense ? "mb-[20px]" : "mb-[30px]";
}

/** The mono uppercase label that *is* the avatar in this design. */
function SpeakerLabel({
  children,
  tone,
  className,
}: {
  children: React.ReactNode;
  tone: "user" | "assistant";
  className?: string;
}) {
  return (
    <span
      className={cn(
        "font-mono text-2xs-plus tracking-speaker uppercase",
        tone === "assistant" ? "text-blue" : "text-muted-fg",
        className,
      )}
    >
      {children}
    </span>
  );
}

export interface UserMessageProps {
  text: string;
  /** `14:22`, or `null` when the message carries no usable timestamp. */
  time: string | null;
  /** Set when this message was routed to a running workflow (§3.10). */
  steer?: SteerRef | null;
  dense?: boolean;
}

export function UserMessage({
  text,
  time,
  steer = null,
  dense = false,
}: UserMessageProps) {
  const label = time === null ? "You" : `You · ${time}`;

  return (
    <article className={messageGapClass(dense)}>
      {steer === null ? (
        <SpeakerLabel tone="user" className="mb-[8px] block">
          {label}
        </SpeakerLabel>
      ) : (
        <div className="mb-[8px] flex items-center gap-[9px]">
          <SpeakerLabel tone="user">{label}</SpeakerLabel>
          <span className="rounded-sm bg-amber-tint px-[6px] py-[2px] font-mono text-2xs tracking-label text-amber-ink">
            {steer.mode === "steer" ? "steer" : "follow-up"} → {steer.label}
          </span>
        </div>
      )}
      <MessageBody text={text} spacing="user" />
    </article>
  );
}

/** `null` while nothing is streaming; otherwise the live SSE phase. */
export type StreamPhase = "thinking" | "streaming" | null;

export interface AssistantMessageProps {
  text: string;
  /** The SSE `done` payload, mapped 1:1 onto the meta line. */
  meta?: AssistantMeta | null;
  streamPhase?: StreamPhase;
  dense?: boolean;
  /** Inline content below the body — the artifact card (§3.13). */
  children?: React.ReactNode;
}

export function AssistantMessage({
  text,
  meta = null,
  streamPhase = null,
  dense = false,
  children,
}: AssistantMessageProps) {
  const metaLine = meta === null ? null : assistantMetaLine(meta);

  return (
    <article className={messageGapClass(dense)}>
      <div className="mb-[8px] flex items-center gap-[9px]">
        <SpeakerLabel tone="assistant">Alpaca</SpeakerLabel>
        {streamPhase === "thinking" ? (
          <ThinkingIndicator />
        ) : (
          metaLine !== null && (
            <span className="font-mono text-2xs-plus text-faint">
              {metaLine}
            </span>
          )
        )}
      </div>
      <MessageBody
        text={text}
        spacing="assistant"
        trailing={streamPhase === "streaming" ? <StreamCaret /> : undefined}
      />
      {children}
    </article>
  );
}
