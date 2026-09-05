/**
 * The transcript model (DESIGN_SPEC §5.1).
 *
 * `buildTranscript` is pure: history rows from `GET /v1/chat/history`, the live
 * SSE turn, the run reports and confirmations this session observed, in one
 * ordered list. Keeping it pure is what makes the streaming lifecycle
 * (thinking → deltas → done) testable without a socket.
 *
 * Two ordering facts drive the design:
 *   * a finished turn invalidates the history query, so for a moment the same
 *     turn exists twice — once live, once persisted. The live copy is dropped
 *     as soon as history carries it (`done.content` is byte-identical to what
 *     was stored), never by a timer;
 *   * nothing links a message to the run or artifacts it produced (GAP-23), so
 *     run reports are session-local: they are rebuilt from the delegation this
 *     client started and the `task_status` frames it saw, and they do not
 *     survive a reload. That is a gap, not a bug in this file.
 */

import type { AssistantMeta } from "@/components/chat";
import type { SteerRef } from "@/components/chat";
import type { Resolution } from "@/components/chat";
import type { RunReportStatus } from "@/components/chat";
import type { AttachmentDisplay, ChatMessage } from "@/lib/api/types";
import type { ChatStreamState } from "@/lib/chat-stream";

/** `/steer …` is the only steering channel there is (GAP-02). */
export const STEER_PREFIX = "/steer ";

export interface ParsedUserContent {
  text: string;
  steered: boolean;
}

export function parseUserContent(content: string): ParsedUserContent {
  if (content.startsWith(STEER_PREFIX)) {
    return { text: content.slice(STEER_PREFIX.length), steered: true };
  }
  return { text: content, steered: false };
}

export interface RunReportData {
  taskId: string;
  title: string;
  status: RunReportStatus;
  /** ISO stamps; the card's duration is the span between them. */
  startedAt: string | null;
  endedAt: string;
  summary: string | null;
  artifactCount: number;
}

export interface ConfirmationEntry {
  requestId: string;
  toolName: string;
  toolArguments: unknown;
  /** From the WS twin of the SSE frame, which carries `agent_id`. */
  agentName: string | null;
  at: string;
}

export interface ResolutionEntry {
  requestId: string;
  resolution: Resolution;
  note: string;
  at: string;
}

/** The optimistic user turn, shown until history catches up. */
export interface PendingTurn {
  /** What the composer displays. */
  text: string;
  /** What was actually POSTed — may carry the `/steer ` prefix. */
  sent: string;
  at: string;
  steer: SteerRef | null;
}

export type StreamPhaseLabel = "thinking" | "streaming" | null;

export interface AttachmentInfo {
  fileId: string;
  /** `null` when only the id is known (`done.attachments_used`). */
  filename: string | null;
  mimeType: string | null;
}

export type TranscriptItem =
  | {
      kind: "user";
      key: string;
      text: string;
      time: string | null;
      steer: SteerRef | null;
    }
  | {
      kind: "assistant";
      key: string;
      text: string;
      meta: AssistantMeta | null;
      streamPhase: StreamPhaseLabel;
      attachments: AttachmentInfo[];
    }
  | { kind: "report"; key: string; report: RunReportData }
  | { kind: "confirmation"; key: string; entry: ConfirmationEntry }
  | { kind: "resolution"; key: string; entry: ResolutionEntry }
  | { kind: "error"; key: string; message: string };

export interface TranscriptInput {
  history: readonly ChatMessage[];
  reports: readonly RunReportData[];
  confirmations: readonly ConfirmationEntry[];
  resolutions: readonly ResolutionEntry[];
  stream: ChatStreamState;
  pending: PendingTurn | null;
  /** Label for the steer pill on history messages that carry the prefix. */
  steerLabel?: string;
}

interface Slot {
  at: number;
  seq: number;
  item: TranscriptItem;
}

function timestamp(value: string | null | undefined): number {
  if (!value) return Number.MAX_SAFE_INTEGER;
  const parsed = new Date(value).getTime();
  return Number.isNaN(parsed) ? Number.MAX_SAFE_INTEGER : parsed;
}

function toAttachments(
  attachments: AttachmentDisplay[] | undefined,
): AttachmentInfo[] {
  return (attachments ?? []).map((attachment) => ({
    fileId: attachment.file_id,
    filename: attachment.filename,
    mimeType: attachment.mime_type,
  }));
}

function messageMeta(message: ChatMessage): AssistantMeta | null {
  const meta: AssistantMeta = {};
  if (message.model !== undefined) meta.model = message.model;
  if (message.duration_ms !== undefined) meta.durationMs = message.duration_ms;
  if (message.tokens_in !== undefined) meta.tokensIn = message.tokens_in;
  if (message.tokens_out !== undefined) meta.tokensOut = message.tokens_out;
  return Object.keys(meta).length === 0 ? null : meta;
}

/** The live SSE phase, mapped onto what a row can show. */
export function streamPhaseLabel(stream: ChatStreamState): StreamPhaseLabel {
  switch (stream.phase) {
    case "opening":
    case "thinking":
      return "thinking";
    case "streaming":
      return "streaming";
    default:
      return null;
  }
}

/** True while the live turn should be rendered as its own assistant row. */
export function showsLiveTurn(
  stream: ChatStreamState,
  history: readonly ChatMessage[],
): boolean {
  if (stream.phase === "idle" || stream.phase === "error") return false;
  if (!stream.terminal) return true;
  // Terminal: keep showing it until the persisted copy arrives.
  const persisted = history.some(
    (message) =>
      message.role === "assistant" && message.content === stream.content,
  );
  return !persisted;
}

/** True while the optimistic user row should still be rendered. */
export function showsPendingTurn(
  pending: PendingTurn | null,
  history: readonly ChatMessage[],
): boolean {
  if (pending === null) return false;
  return !history.some(
    (message) => message.role === "user" && message.content === pending.sent,
  );
}

export function buildTranscript(input: TranscriptInput): TranscriptItem[] {
  const {
    history,
    reports,
    confirmations,
    resolutions,
    stream,
    pending,
    steerLabel = "the active workflow",
  } = input;

  const slots: Slot[] = [];
  let seq = 0;
  const push = (at: number, item: TranscriptItem) => {
    slots.push({ at, seq: seq++, item });
  };

  for (const message of history) {
    if (message.role === "system") continue;
    const at = timestamp(message.created_at);
    const body = message.display_text ?? message.content;

    if (message.role === "user") {
      const parsed = parseUserContent(body);
      push(at, {
        kind: "user",
        key: `m${message.id}`,
        text: parsed.text,
        time: message.created_at,
        steer: parsed.steered ? { mode: "steer", label: steerLabel } : null,
      });
      continue;
    }

    push(at, {
      kind: "assistant",
      key: `m${message.id}`,
      text: body,
      meta: messageMeta(message),
      streamPhase: null,
      attachments: toAttachments(message.attachments),
    });
  }

  for (const report of reports) {
    push(timestamp(report.endedAt), {
      kind: "report",
      key: `r${report.taskId}`,
      report,
    });
  }

  for (const entry of confirmations) {
    push(timestamp(entry.at), {
      kind: "confirmation",
      key: `c${entry.requestId}`,
      entry,
    });
  }

  for (const entry of resolutions) {
    push(timestamp(entry.at), {
      kind: "resolution",
      key: `x${entry.requestId}`,
      entry,
    });
  }

  if (showsPendingTurn(pending, history) && pending !== null) {
    push(timestamp(pending.at), {
      kind: "user",
      key: "pending-user",
      text: pending.text,
      time: pending.at,
      steer: pending.steer,
    });
  }

  if (showsLiveTurn(stream, history)) {
    const meta: AssistantMeta | null =
      stream.result === null
        ? null
        : {
            model: stream.result.model,
            durationMs: stream.result.duration_ms,
            tokensIn: stream.result.tokens_in,
            tokensOut: stream.result.tokens_out,
          };
    const attachments =
      stream.result?.attachments_used?.map((fileId) => ({
        fileId,
        filename: null,
        mimeType: null,
      })) ?? [];

    push(Number.MAX_SAFE_INTEGER, {
      kind: "assistant",
      key: `live-${stream.streamId ?? "0"}`,
      text: stream.content,
      meta,
      streamPhase: streamPhaseLabel(stream),
      attachments,
    });
  }

  if (stream.phase === "error" && stream.error !== null) {
    push(Number.MAX_SAFE_INTEGER, {
      kind: "error",
      key: `e-${stream.streamId ?? "0"}`,
      message: stream.error.message,
    });
  }

  slots.sort((a, b) => (a.at === b.at ? a.seq - b.seq : a.at - b.at));
  return slots.map((slot) => slot.item);
}
