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
 *   * a stored message now names the run it started or reported on, and a
 *     report names the files that run produced (GAP-23, closed): the run pill
 *     and the artifact chips are read straight off history and survive a
 *     reload. The recap *card* is still session-local — it is drawn from the
 *     `task_status` frame this client saw, which carries the status, the
 *     duration and the summary that no stored message does.
 */

import type { AssistantMeta } from "@/components/chat";
import type { SkippedAttachment } from "@/components/chat";
import type { SteerRef } from "@/components/chat";
import type { Resolution } from "@/components/chat";
import type { RunReportStatus } from "@/components/chat";
import type {
  AttachmentDisplay,
  ChatMessage,
  MessageArtifact,
} from "@/lib/api/types";
import type { ChatStreamState } from "@/lib/chat-stream";
import { timestampMs } from "@/lib/time";

/**
 * The chat prefix the orchestrator strips. This client no longer *sends* it —
 * a steer is `POST /v1/tasks/{id}/steer` now (GAP-02, closed) — but stored
 * history still carries it: from the CLI and Telegram, whose only steering
 * channel it remains, and from turns this GUI sent before the route existed.
 */
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

/**
 * One `artifact_written` frame, as the transcript shows it.
 *
 * Session-local like the run reports: the frame carries a *version*, which the
 * card prints and no stored link records. A file the run produced comes back
 * after a reload as a chip on the completion report (GAP-23, closed); this
 * card is the moment it was written. The Library is where the file is
 * permanent either way.
 */
export interface WrittenArtifact {
  artifactId: string;
  /** The head file's own name, as the daemon wrote it. */
  name: string;
  /** The daemon's snake_case `ArtifactKind` spelling. */
  kind: string;
  version: number;
  /** `null` for a loose artifact — a chat turn that ran no workflow. */
  taskId: string | null;
  at: string;
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
  /**
   * The files this turn carried (T5), so the row shows them the moment it is
   * sent and not only once history comes back with the links.
   */
  attachments: AttachmentInfo[];
}

/**
 * A steer this client pushed through `POST /v1/tasks/{id}/steer`, or a
 * follow-up it queued through `POST /v1/lanes/{lane_key}/followups`.
 *
 * Session-local by construction, and for a reason the run reports do not share:
 * neither is a chat turn, so the daemon stores no message for either and a
 * reload has nothing to rebuild from. The design still shows both as user
 * messages carrying the `steer → {run}` / `follow-up → {run}` pill (§5.1.4), so
 * the row is drawn from what this client sent — never inferred from a
 * `workflow_steered` or `followup_queued` frame, neither of which carries text.
 */
export interface SteerEntry {
  /** Client-side id; only ever a React key. */
  id: string;
  text: string;
  /** Which pill the row wears — the composer mode it was sent in. */
  mode: SteerRef["mode"];
  /** The run's short title — the pill's label. */
  label: string;
  at: string;
}

export type StreamPhaseLabel = "thinking" | "streaming" | null;

export interface AttachmentInfo {
  fileId: string;
  /** `null` when only the id is known (`done.attachments_used`). */
  filename: string | null;
  mimeType: string | null;
  /**
   * The daemon's snake_case `ArtifactKind`, when the server named it (an
   * artifact link does; a file id does not). `null` falls back to the badge
   * the filename and mime type imply.
   */
  kind: string | null;
}

export type TranscriptItem =
  | {
      kind: "user";
      key: string;
      text: string;
      time: string | null;
      steer: SteerRef | null;
      /**
       * The files this turn carried in (T5) — `role='attachment'` links on the
       * stored message, or the chips the composer just sent.
       *
       * They are shown *as files*. The daemon also writes a text rendering of
       * them into the row's `display_text` ("…\n[Attachments: notes.txt]") for
       * surfaces that can only print a string; this window is not one, and
       * printing that suffix as if the person had typed it is what T5 found.
       */
      attachments: AttachmentInfo[];
    }
  | {
      kind: "assistant";
      key: string;
      text: string;
      meta: AssistantMeta | null;
      streamPhase: StreamPhaseLabel;
      /**
       * The model's live reasoning (S2) — only ever on the live row, and only
       * while it is still thinking. Empty for every stored message: nothing
       * persists it.
       */
      reasoning: string;
      /** Files the turn carried in (`role='attachment'`). */
      attachments: AttachmentInfo[];
      /**
       * Files the turn carried in that the model never received (U3), from
       * `done.attachments_skipped`.
       *
       * Live only: the daemon stores no skip on the message, so a reloaded
       * transcript has nothing to rebuild the note from and this is `[]` for
       * every stored row.
       */
      skipped: SkippedAttachment[];
      /** Files the turn's run produced (`role='artifact'`, GAP-23). */
      artifacts: AttachmentInfo[];
      /** The run this turn started or reported on — `null` for plain chat. */
      runId: string | null;
    }
  | { kind: "report"; key: string; report: RunReportData }
  | { kind: "artifact"; key: string; entry: WrittenArtifact }
  | { kind: "confirmation"; key: string; entry: ConfirmationEntry }
  | { kind: "resolution"; key: string; entry: ResolutionEntry }
  | { kind: "error"; key: string; message: string };

export interface TranscriptInput {
  history: readonly ChatMessage[];
  reports: readonly RunReportData[];
  /** Files this session watched an agent write, newest last. */
  artifacts: readonly WrittenArtifact[];
  confirmations: readonly ConfirmationEntry[];
  resolutions: readonly ResolutionEntry[];
  /** Steers this client pushed to a run's own `/steer` route, newest last. */
  steers: readonly SteerEntry[];
  stream: ChatStreamState;
  pending: PendingTurn | null;
  /** Label for the steer pill on history messages that carry the prefix. */
  steerLabel?: string;
  /**
   * `file_id → filename` for uploads this window made, so a skipped
   * attachment reads as a name instead of an id (U3). Unknown ids fall back to
   * the id itself — never to a guess.
   */
  attachmentNames?: Readonly<Record<string, string>>;
}

interface Slot {
  at: number;
  seq: number;
  item: TranscriptItem;
}

/**
 * Where a row sorts. Unreadable or absent sorts last, which is where a row
 * with no time belongs in a transcript.
 *
 * `timestampMs` and not `Date.parse`: a persisted message carries SQLite's
 * zone-less UTC, and reading it as local time put every stored row *hours
 * ahead* of the `Z`-stamped rows this client makes itself — so the message the
 * user had just sent sorted above the entire conversation and read as lost
 * until the turn finished (G5, a symptom of G3).
 */
function timestamp(value: string | null | undefined): number {
  return timestampMs(value) ?? Number.MAX_SAFE_INTEGER;
}

function toAttachments(
  attachments: AttachmentDisplay[] | undefined,
): AttachmentInfo[] {
  return (attachments ?? []).map((attachment) => ({
    fileId: attachment.file_id,
    filename: attachment.filename,
    mimeType: attachment.mime_type,
    kind: null,
  }));
}

/** A message's `role='artifact'` links, as the same card reads them. */
function toArtifacts(
  artifacts: MessageArtifact[] | undefined,
): AttachmentInfo[] {
  return (artifacts ?? []).map((artifact) => ({
    fileId: artifact.id,
    filename: artifact.name,
    // The artifact link names the file, not its bytes; the kind the daemon
    // stored is a better badge than anything the mime type would guess.
    mimeType: null,
    kind: artifact.kind,
  }));
}

function messageMeta(message: ChatMessage): AssistantMeta | null {
  const meta: AssistantMeta = {};
  // `message.model` is `string | null | undefined` on the wire: `null` for
  // every row the daemon has nothing to say about (pre-GAP-13 history, and
  // any template answer that never ran a model). Only a real string counts.
  if (typeof message.model === "string") meta.model = message.model;
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
    artifacts,
    confirmations,
    resolutions,
    steers,
    stream,
    pending,
    steerLabel = "the active workflow",
    attachmentNames = {},
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
      // `content`, never `display_text` (T5). The two differ on exactly one
      // kind of row — a turn that carried files, where `display_text` is the
      // text the person typed plus `\n[Attachments: a.txt, b.png]`, built by
      // the daemon for a client that can only print a string. This one draws
      // the links instead, so the suffix would be the same fact told twice,
      // the second time as if the person had typed it.
      const parsed = parseUserContent(message.content);
      push(at, {
        kind: "user",
        key: `m${message.id}`,
        text: parsed.text,
        time: message.created_at,
        steer: parsed.steered ? { mode: "steer", label: steerLabel } : null,
        attachments: toAttachments(message.attachments),
      });
      continue;
    }

    push(at, {
      kind: "assistant",
      key: `m${message.id}`,
      text: body,
      meta: messageMeta(message),
      streamPhase: null,
      // A stored message has none: the daemon keeps reasoning out of what it
      // writes, so there is nothing to replay.
      reasoning: "",
      attachments: toAttachments(message.attachments),
      // Nothing persists a skip, so a stored row can only ever say "none".
      skipped: [],
      artifacts: toArtifacts(message.artifacts),
      runId: message.task_id ?? null,
    });
  }

  for (const report of reports) {
    push(timestamp(report.endedAt), {
      kind: "report",
      key: `r${report.taskId}`,
      report,
    });
  }

  for (const entry of artifacts) {
    // The version is part of the key: superseding a file is a second event and
    // deserves its own card, not a silently mutated one.
    push(timestamp(entry.at), {
      kind: "artifact",
      key: `a-${entry.artifactId}-${entry.version}`,
      entry,
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

  // A steer or a queued follow-up is a user message in the design (§5.1.4) even
  // though neither went down the chat channel, so both take the same row — with
  // the pill naming the run it was addressed to and which of the two it was.
  for (const entry of steers) {
    push(timestamp(entry.at), {
      kind: "user",
      key: `s${entry.id}`,
      text: entry.text,
      time: entry.at,
      steer: { mode: entry.mode, label: entry.label },
      // Neither route takes attachments, so a steer or a follow-up never
      // carries one.
      attachments: [],
    });
  }

  if (showsPendingTurn(pending, history) && pending !== null) {
    push(timestamp(pending.at), {
      kind: "user",
      key: "pending-user",
      text: pending.text,
      time: pending.at,
      steer: pending.steer,
      attachments: pending.attachments,
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
        kind: null,
      })) ?? [];
    // Disjoint from `attachments_used` since U3 — a withheld attachment is no
    // longer listed as used, so a file appears as a chip or in the note, never
    // as both.
    const skipped: SkippedAttachment[] = (
      stream.result?.attachments_skipped ?? []
    ).map((entry) => ({
      fileId: entry.id,
      filename: attachmentNames[entry.id] ?? null,
      reason: entry.reason,
    }));

    push(Number.MAX_SAFE_INTEGER, {
      kind: "assistant",
      key: `live-${stream.streamId ?? "0"}`,
      text: stream.content,
      meta,
      streamPhase: streamPhaseLabel(stream),
      reasoning: stream.reasoning,
      attachments,
      skipped,
      // The live turn's own delegation reaches the transcript as a report card,
      // and the pill arrives with the row when history catches up.
      artifacts: [],
      runId: null,
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
