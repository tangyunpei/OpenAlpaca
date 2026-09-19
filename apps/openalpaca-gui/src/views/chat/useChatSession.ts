/**
 * The chat view's glue: history, the live SSE turn, confirmations, run reports.
 *
 * Everything stateful about a chat lane lives here so `ChatView` stays a
 * layout. The streaming machine itself is *not* re-implemented — `useChatStream`
 * owns the SSE contract; this hook only decorates it with the things
 * the design shows around a turn.
 *
 * Honest wiring, gap by gap:
 *   * Steering (GAP-02, closed) is `POST /v1/tasks/{id}/steer`: it addresses
 *     the run the composer is aimed at, not whatever the lane happens to be
 *     running, and it answers `accepted`/`inbox_depth` synchronously. It is
 *     *not* a chat turn — nothing is persisted for it — so the transcript row
 *     is session-local, like the run reports beside it. Every refusal code
 *     gets its own toast; a steer that did not land never looks like one that
 *     did.
 *   * Queueing a follow-up (GAP-03, closed) is
 *     `POST /v1/lanes/{lane_key}/followups`: it parks the text on the lane and
 *     the daemon runs it when the current workflow finalizes. Like a steer it
 *     is not a chat turn, so its transcript row is session-local; unlike a
 *     steer it is addressed at the *lane*, and `source_task_id` records which
 *     run it was queued behind. Every refusal code gets its own toast.
 *   * `Always allow` sends `approval_scope: "entire_tool"`, which the daemon
 *     now honours for the rest of the session (GAP-01, closed) — the toast
 *     uses the design's own copy (§4.4): `{tool} added to the allowlist — it
 *     won't ask again`.
 *   * **GAP-23 (closed)** a stored message carries the run it started or
 *     reported on, and a report carries the files its run produced, so the run
 *     pill and the artifact chips are read off history. The recap *card* is
 *     still built here from the delegation this client started plus the
 *     `task_status` frames it saw: the status, duration and summary it prints
 *     are on those frames and on no stored message.
 *   * The blocked run is the `task_id` the confirmation frame carries, with
 *     `agent_status`'s `agent_id → current_task_id` as the fallback for a
 *     frame that has none. When neither answers it stays `null` rather than
 *     being guessed.
 *
 * Three rules keep this window honest about *whose* conversation it is showing:
 *   * the transcript is the conversation's **tail**, read in two calls, because
 *     `GET /v1/chat/history` pages oldest-first (R78);
 *   * everything session-local — reports, artifacts, steers, resolutions, the
 *     finished SSE turn — is cleared when the daemon reports a different
 *     conversation, so nothing follows the user out of one and into the next;
 *   * `/v1/events` is daemon-wide, so run and confirmation frames are filtered
 *     on the lane they name. A confirmation from another lane must never block
 *     this composer: answering one with `Always allow` would widen a foreign
 *     agent's run.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  executedResolutionNote,
  formatDurationMs,
  resolutionNote,
  settlingRun,
  shortTitle,
  type Resolution,
  type ToolRun,
} from "@/components/chat";
import { toUiStatus, type UiStatus } from "@/components/ui";
import { useAgentTemplates } from "@/hooks/useAgents";
import {
  useChatHistory,
  useChatStream,
  usePendingConfirmations,
} from "@/hooks/useChat";
import { useServerEvent } from "@/hooks/useDaemonEvents";
import {
  useCancelFollowup,
  useFollowups,
  useQueueFollowup,
} from "@/hooks/useFollowups";
import { useTasks } from "@/hooks/useTasks";
import { followupErrorMessage, type FollowupRecord } from "@/lib/api/followups";
import { sessionErrorMessage } from "@/lib/api/sessions";
import { steerErrorMessage, steerTask } from "@/lib/api/tasks";
import type { ApprovalScope } from "@/lib/api/types";
import { ApiError } from "@/lib/http";
import { isTerminalRunStatus } from "@/lib/query-client";
import { useProjectStore, workspaceOption } from "@/stores/project";
import { useSessionSelection } from "@/stores/session";
import { useUiStore, type ComposerMode } from "@/stores/ui";

import {
  buildTranscript,
  type ConfirmationEntry,
  type PendingTurn,
  type ResolutionEntry,
  type RunReportData,
  type SteerEntry,
  type TranscriptItem,
  type WrittenArtifact,
} from "./transcript-model";

/**
 * Whether a confirmation the daemon reports is this window's to answer.
 *
 * One rule for both ways a prompt reaches this view — the live
 * `tool_confirmation_requested` frame and the `GET /v1/chat/confirmations`
 * snapshot S9 added — because accepting one is what blocks this composer and
 * arms Enter-approve, and answering a foreign run's prompt from here (with
 * `Always allow`, worse) is exactly what the lane filter exists to prevent.
 *
 * Three things make a prompt this window's, in the order they are trusted: it
 * is this stream's own, it belongs to a run this lane started, or its lane is
 * this one. A `lane_key: null` prompt is **kept**: `SandboxPolicy` carries a
 * lane only on the main-loop policy, so every confirmation raised inside a
 * workflow arrives with none, and dropping those would leave the GUI's own
 * background runs unanswerable. The snapshot's rows carry no `stream_id` at
 * all, which is why it is optional here.
 */
export function confirmationBelongsHere(
  frame: {
    stream_id?: string | null;
    task_id: string | null;
    lane_key: string | null;
  },
  context: {
    streamId: string | null;
    laneKey: string | null;
    startedHere: (taskId: string) => boolean;
  },
): boolean {
  const streamId = frame.stream_id ?? null;
  if (streamId !== null && streamId === context.streamId) return true;
  if (frame.task_id !== null && context.startedHere(frame.task_id)) return true;
  return frame.lane_key === null || frame.lane_key === context.laneKey;
}

/**
 * What to call the agent a confirmation card is waiting on — one rule for both
 * ways a prompt reaches this view (G10).
 *
 * The two paths used to disagree about the same prompt: live it read "Lead
 * Agent", and the card restored from `GET /v1/chat/confirmations` after a
 * reload read "lead_agent". Neither path was wrong — they had different
 * information. The name came from the `agent_status` map, which is built from
 * *frames*, and frames are live-only: a window that opened after the agent was
 * spawned has none, so it fell through to the raw id.
 *
 * Both prompts carry the same thing — the **template** id ("The agent template
 * that asked", `routes/chat_types.rs`) — so the daemon's own template list is
 * the name table that survives a reload, and it is asked first. The live map
 * stays behind it for the ids it does answer: a non-singleton instance
 * (`code_agent::a1b2c3d4`) is in no template list, and a window whose template
 * list has not loaded yet still has whatever it has seen. The raw id is the
 * last resort, never a guess: `general_agent` is called "General Purpose
 * Agent", so there is no titlecasing rule to invent here.
 */
export function agentDisplayName(
  agentId: string,
  templateNames: ReadonlyMap<string, string>,
  instanceName?: string,
): string {
  const template = templateNames.get(agentId);
  if (template !== undefined && template !== "") return template;
  // `AgentStatusChanged.name` is empty when the instance could not be
  // resolved (GAP-07), and an empty name is not a name.
  if (instanceName !== undefined && instanceName !== "") return instanceName;
  return agentId;
}

/** Constant identities: `useServerEvent` keys its subscription off the list. */
const RUN_EVENTS = ["workflow_started", "task_status"] as const;
const AGENT_EVENTS = ["agent_status"] as const;
const CONFIRM_EVENTS = ["tool_confirmation_requested"] as const;
const TOOL_EVENTS = ["tool_executed"] as const;
const ARTIFACT_EVENTS = ["artifact_written"] as const;
const SESSION_EVENTS = ["session_changed"] as const;

/**
 * How many messages one read of `GET /v1/chat/history` asks for.
 *
 * It is a **window**, not a bound on the conversation: the route pages
 * `ORDER BY created_at ASC` with `limit`/`offset`, so asking without an offset
 * returns the conversation's *opening*. Two rows land per turn, nothing prunes
 * or rotates a session by size, and ~50 turns therefore push every recent turn
 * out of a head page — which is what the transcript is built from.
 */
const HISTORY_LIMIT = 100;

interface StartedRun {
  title: string;
  startedAt: string;
}

interface ConfirmationMeta {
  at: string;
  /**
   * Who asked, as the daemon names them — the agent **template** id on both
   * paths. The display name is resolved from it at render time by
   * `agentDisplayName`, not frozen here: the template list can answer after
   * the card is drawn, and a name frozen from an empty map is what made the
   * live and restored cards of one prompt disagree (G10).
   */
  agentId: string | null;
  /**
   * The run the daemon is blocked on, straight off the frame. `null` for a
   * confirmation raised outside a workflow — the main loop's own — and for a
   * daemon too old to carry it, in which case the `agent_status` map is the
   * fallback.
   */
  taskId: string | null;
}

interface AgentRecord {
  name: string;
  taskId: string | null;
}

export interface ActiveRun {
  id: string;
  title: string;
  status: UiStatus;
}

export interface ChatSession {
  items: TranscriptItem[];
  historyLoading: boolean;
  historyError: Error | null;
  laneKey: string | null;
  /**
   * The conversation this transcript came from, as the daemon reported it —
   * `null` on a lane that has never held a turn. Not a local memory of what
   * was clicked: the sidebar highlights this.
   */
  sessionId: string | null;

  draft: string;
  setDraft: (value: string) => void;
  send: () => void;
  sending: boolean;
  sendError: string | null;

  blocked: boolean;
  /** The tool the daemon is waiting on, if any. */
  pendingToolName: string | null;
  /** The run holding that confirmation — `null` when it cannot be resolved. */
  blockedRunId: string | null;
  answering: boolean;
  approve: () => void;
  deny: () => void;
  alwaysAllow: () => void;

  /**
   * The snapshot of prompts a run is waiting on could not be read (V9).
   *
   * Live frames are unaffected — this is only the seed S9 added — but a window
   * that opened while a background run was blocked then shows no card and the
   * run sits on the prompt until it times out. That is worth a line, for the
   * same reason `historyError` is: a silent failure here looks exactly like
   * "nothing is waiting".
   */
  confirmationsError: Error | null;

  activeRuns: ActiveRun[];
  steer: { mode: ComposerMode; label: string } | null;

  /** The lane's pending follow-up queue, oldest first (claim order). */
  followups: FollowupRecord[];
  /** The queue could not be read; items may still be pending. */
  followupsError: Error | null;
  cancelFollowup: (followupId: number) => void;
  /** The row a cancel is in flight for. */
  cancellingFollowupId: number | null;
}

/**
 * The report card's word for a run that stopped.
 *
 * Which statuses stop a run is `isTerminalRunStatus`'s to say — the same list
 * `query-client.ts` invalidates the transcript on, so a status added there
 * cannot quietly stop carding here. `interrupted` (§5.6b, the daemon went away
 * mid-run) is terminal and must not read as "failed".
 */
function reportStatus(status: string): RunReportData["status"] {
  if (status === "completed") return "done";
  if (status === "cancelled") return "cancelled";
  if (status === "interrupted") return "interrupted";
  return "failed";
}

export function useChatSession(): ChatSession {
  // The conversation the sidebar has resumed, if any. `null` — the default —
  // asks the daemon for the lane's active session, which is the state R48
  // needs in order to open a new conversation when the project changes.
  const selectedSessionId = useSessionSelection((s) => s.selectedId);
  const sessionArg =
    selectedSessionId === null ? {} : { sessionId: selectedSessionId };

  /**
   * The transcript is the conversation's **tail**, in two calls (ruling R78).
   *
   * `GET /v1/chat/history` pages oldest-first, so one call with no offset is
   * the conversation's opening — for a long-lived conversation, turns 1–50,
   * for ever, with the recent ones never shown and the previous turn vanishing
   * as the next starts (`showsLiveTurn` retires a row only when the fetched
   * page contains it). This mirrors what the CLI's `chat --resume` does with
   * the same route: the first call is the length probe, and once `total` is
   * known the second asks for the last page of it. The route contract is
   * unchanged — no new parameter, no new meaning for `offset` — and the extra
   * round trip happens only on a conversation long enough to need it.
   */
  const historyProbe = useChatHistory({ limit: HISTORY_LIMIT, ...sessionArg });
  const tailOffset = Math.max(
    0,
    (historyProbe.data?.total ?? 0) - HISTORY_LIMIT,
  );
  const historyTail = useChatHistory(
    { limit: HISTORY_LIMIT, offset: tailOffset, ...sessionArg },
    { enabled: tailOffset > 0 },
  );
  // Until the tail lands the probe *is* the tail, because a conversation
  // shorter than one page has only one page.
  const history =
    tailOffset > 0 && historyTail.data !== undefined
      ? historyTail
      : historyProbe;

  /**
   * The lane's confirmations, and nobody else's.
   *
   * `/v1/events` forwards every frame to every client, so a confirmation from
   * a scheduled skill, a connector or a wake turn used to block this composer
   * and arm Enter-approve — and an `entire_tool` answer from here would widen
   * that foreign run for the rest of its session. Three things make a frame
   * this view's, in the order they are trusted: it is this stream's own
   * (`stream_id`), it belongs to a run this lane started (`task_id`), or its
   * lane is this one. A `lane_key: null` frame is **kept**: `SandboxPolicy`
   * carries a lane only on the main-loop policy, so every confirmation raised
   * inside a workflow — this lane's own included — arrives with none, and
   * dropping those would leave the GUI's background runs unanswerable.
   */
  const laneKeyRef = useRef<string | null>(null);
  /** Runs this client started, so a `task_status` frame can be reported. */
  const started = useRef(new Map<string, StartedRun>());
  /** `agent_id → { name, current_task_id }` — the only run mapping on the wire. */
  const agents = useRef(new Map<string, AgentRecord>());
  /**
   * `template id → display name`, the half of the confirmation card's copy
   * that outlives a reload (G10). A cache read for a window that has already
   * opened Settings → Agents, one small `GET /v1/agent-templates` otherwise.
   */
  const agentTemplates = useAgentTemplates();
  const templateNames = useMemo(() => {
    const names = new Map<string, string>();
    for (const template of agentTemplates.data ?? [])
      names.set(template.id, template.name);
    return names;
  }, [agentTemplates.data]);
  /**
   * The last few `tool_executed` frames, for the card that has not been drawn
   * yet (G6). The broker releases the tool the moment the answer is posted, so
   * the outcome regularly beats this client's own `onSuccess`.
   */
  const toolRuns = useRef<ToolRun[]>([]);

  const stream = useChatStream({
    accepts: (event, streamId) =>
      confirmationBelongsHere(event, {
        streamId,
        laneKey: laneKeyRef.current,
        startedHere: (taskId) => started.current.has(taskId),
      }),
  });
  const activeTasks = useTasks({ status: "active" });

  /** The lane this session is on — `null` until its first turn has a key. */
  const laneKey = history.data?.lane_key ?? stream.state.laneKey;
  laneKeyRef.current = laneKey;

  /** A frame from another lane is another lane's business. */
  const foreignLane = (frameLane: string | null): boolean =>
    laneKey !== null && frameLane !== null && frameLane !== laneKey;

  const followupQueue = useFollowups(laneKey);
  const { mutateAsync: queueFollowup } = useQueueFollowup();
  const { mutate: cancelFollowupMutation, isPending: cancelPending } =
    useCancelFollowup();
  const [cancellingFollowupId, setCancellingFollowupId] = useState<
    number | null
  >(null);

  const model = useUiStore((s) => s.model);
  const steerTargetRunId = useUiStore((s) => s.steerTargetRunId);
  const composerMode = useUiStore((s) => s.composerMode);
  const clearSteerTarget = useUiStore((s) => s.clearSteerTarget);
  const showToast = useUiStore((s) => s.showToast);
  // §4.7 item 2: the project the turn belongs to, sent as `x-workspace-path`.
  // `null` (no project chosen) sends no header at all.
  const projectPath = useProjectStore((s) => s.path);

  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState<PendingTurn | null>(null);
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const [reports, setReports] = useState<RunReportData[]>([]);
  const [artifacts, setArtifacts] = useState<WrittenArtifact[]>([]);
  const [resolutions, setResolutions] = useState<ResolutionEntry[]>([]);
  const [steers, setSteers] = useState<SteerEntry[]>([]);
  const [confirmationMeta, setConfirmationMeta] = useState<
    Record<string, ConfirmationMeta>
  >({});

  useServerEvent(AGENT_EVENTS, (event) => {
    if (event.type !== "agent_status") return;
    agents.current.set(event.agent_id, {
      name: event.name,
      taskId: event.current_task_id,
    });
  });

  /** Add one run report, once — the same run must not card twice. */
  const pushReport = useCallback((report: RunReportData) => {
    setReports((current) =>
      current.some((entry) => entry.taskId === report.taskId)
        ? current
        : [...current, report],
    );
  }, []);

  useServerEvent(RUN_EVENTS, (event) => {
    if (event.type === "workflow_started") {
      // The frame carries its lane and the socket carries every lane's, so a
      // run started by a scheduled skill or a connector is dropped here rather
      // than entering `started` — which is also what keeps its `task_status`
      // and `artifact_written` frames out of this transcript.
      if (foreignLane(event.lane_key)) return;
      started.current.set(event.task_id, {
        title: event.title,
        startedAt: event.ts,
      });
      return;
    }
    if (event.type !== "task_status") return;
    if (!isTerminalRunStatus(event.status)) return;

    // A finished run is not waiting on anything any more (G1). The daemon
    // publishes no frame for a prompt that timed out or was answered
    // elsewhere, so this is what retires a card the run has moved past —
    // before the `started` check below, because a card can outlive this
    // client's memory of who started the run.
    for (const [requestId, meta] of Object.entries(confirmationMeta)) {
      if (meta.taskId === event.task_id) stream.dismissConfirmation(requestId);
    }

    const origin = started.current.get(event.task_id);
    // Only report workflows this lane actually started — a foreign run's card
    // has no place in this lane.
    if (origin === undefined) return;
    started.current.delete(event.task_id);

    const title = event.title !== "" ? event.title : origin.title;
    pushReport({
      taskId: event.task_id,
      title: title === "" ? "Background workflow" : title,
      status: reportStatus(event.status),
      startedAt: origin.startedAt,
      endedAt: event.ts,
      summary: event.outcome_summary ?? event.result_summary,
      artifactCount: event.artifact_count ?? 0,
    });
  });

  /**
   * A run the daemon never got to finish (§5.6b).
   *
   * The boot sweep marks a run interrupted and announces it on
   * `session_changed{status: "interrupted", task_id}` — not on `task_status`,
   * which is why the `interrupted` card the report model has drawn since Phase
   * 7 was unreachable from this view. The reachable case is a daemon that
   * restarts under a window that stayed open: `started` still holds the run, so
   * the card lands in the conversation that launched it and nowhere else.
   */
  useServerEvent(SESSION_EVENTS, (event) => {
    if (event.type !== "session_changed") return;
    if (event.status !== "interrupted" || event.task_id === null) return;
    if (foreignLane(event.lane_key)) return;

    const origin = started.current.get(event.task_id);
    if (origin === undefined) return;
    started.current.delete(event.task_id);

    pushReport({
      taskId: event.task_id,
      title: origin.title === "" ? "Background workflow" : origin.title,
      status: "interrupted",
      startedAt: origin.startedAt,
      endedAt: event.ts,
      // The sweep reports no outcome — there is none — and the card's own copy
      // says what interrupted means.
      summary: null,
      artifactCount: 0,
    });
  });

  /**
   * A file an agent wrote, shown inline. Two frames belong to this lane: a
   * loose artifact (no run — the main loop wrote it for this conversation) and
   * one from a workflow this lane started. A foreign run's file belongs to that
   * run's lane, and lands in the Library either way.
   *
   * The run-linked half is exact now that `started` holds only this lane's
   * runs. The loose half cannot be: `ArtifactWritten` carries no `lane_key`
   * (`crates/openalpaca_api/src/events/mod.rs`), so a loose artifact written
   * for another lane's main loop is indistinguishable from one written for
   * this conversation, and it is shown. That is the honest side of the trade —
   * a file that exists, attributed a little too widely — and it closes the day
   * the frame carries a lane.
   */
  useServerEvent(ARTIFACT_EVENTS, (event) => {
    if (event.type !== "artifact_written") return;
    if (event.task_id !== null && !started.current.has(event.task_id)) return;
    setArtifacts((current) =>
      current.some(
        (entry) =>
          entry.artifactId === event.artifact_id &&
          entry.version === event.version,
      )
        ? current
        : [
            ...current,
            {
              artifactId: event.artifact_id,
              name: event.name,
              kind: event.kind,
              version: event.version,
              taskId: event.task_id,
              at: event.ts,
            },
          ],
    );
  });

  useServerEvent(CONFIRM_EVENTS, (event) => {
    if (event.type !== "tool_confirmation_requested") return;
    setConfirmationMeta((current) =>
      current[event.request_id] !== undefined
        ? current
        : {
            ...current,
            [event.request_id]: {
              at: event.ts,
              agentId: event.agent_id,
              taskId: event.task_id,
            },
          },
    );
  });

  /**
   * The prompts a run is waiting on right now (S9), seeded into the same
   * session-level list the live frames feed.
   *
   * The frames are live-only — there is no replay — so a window that opened
   * after a prompt was raised showed no card at all and the run sat on it
   * until the 300 s timeout. That is every reload, every second window, and
   * every restart of this app while a background run is blocked. The snapshot
   * closes it on load, and on reconnect: a resync invalidates the whole cache,
   * so this query is re-read the moment the socket comes back.
   *
   * A prompt this window has already seen is skipped — `confirmationMeta` is
   * the record of that, and it survives a card being retired (G1), so a
   * dismissed card is not resurrected by the next poll.
   *
   * A read that **failed** is carried out as `confirmationsError` rather than
   * swallowed (V9): the view says so where it says a failed history, because
   * a snapshot nobody could read looks from here exactly like a snapshot with
   * nothing in it. The live frames are untouched by it.
   */
  const pendingConfirmations = usePendingConfirmations();
  const pendingRows = pendingConfirmations.data;
  const confirmationMetaRef = useRef(confirmationMeta);
  confirmationMetaRef.current = confirmationMeta;
  const adoptConfirmation = stream.adoptConfirmation;

  useEffect(() => {
    if (pendingRows === undefined) return;
    const fresh = pendingRows.filter(
      (row) =>
        confirmationMetaRef.current[row.request_id] === undefined &&
        confirmationBelongsHere(row, {
          streamId: null,
          laneKey: laneKeyRef.current,
          startedHere: (taskId) => started.current.has(taskId),
        }),
    );
    if (fresh.length === 0) return;

    for (const row of fresh) {
      adoptConfirmation({
        request_id: row.request_id,
        tool_name: row.tool_name,
        tool_arguments: row.tool_arguments,
      });
    }
    setConfirmationMeta((current) => {
      const next = { ...current };
      for (const row of fresh) {
        if (next[row.request_id] !== undefined) continue;
        next[row.request_id] = {
          // The daemon's own clock, so a card seeded after a reload sorts
          // where the prompt actually happened rather than at "now".
          at: row.raised_at,
          agentId: row.agent_id,
          taskId: row.task_id,
        };
      }
      return next;
    });
  }, [pendingRows, laneKey, adoptConfirmation]);

  useServerEvent(TOOL_EVENTS, (event) => {
    if (event.type !== "tool_executed") return;
    // Remembered whether or not a card is waiting for it: the card may not
    // exist yet, and when it is made it looks back here (G6). Bounded — this
    // is a settling window, not a log.
    toolRuns.current = [
      ...toolRuns.current.slice(-19),
      {
        toolName: event.tool_name,
        success: event.success,
        duration: formatDurationMs(event.duration_ms),
        atMs: Date.now(),
      },
    ];
    setResolutions((current) =>
      current.map((entry) =>
        entry.resolution === "approved" &&
        entry.note.startsWith(`${event.tool_name} approved · waiting`)
          ? {
              ...entry,
              note: executedResolutionNote(
                event.tool_name,
                event.success,
                formatDurationMs(event.duration_ms),
              ),
            }
          : entry,
      ),
    );
  });

  const activeRuns: ActiveRun[] = useMemo(
    () =>
      (activeTasks.data ?? []).map((task) => ({
        id: task.id,
        title: task.title,
        status: toUiStatus(task.status),
      })),
    [activeTasks.data],
  );

  const steerRun = useMemo(
    () => activeRuns.find((run) => run.id === steerTargetRunId) ?? null,
    [activeRuns, steerTargetRunId],
  );

  const steer =
    steerTargetRunId === null
      ? null
      : {
          mode: composerMode,
          label: shortTitle(steerRun?.title ?? steerTargetRunId),
        };

  const confirmations: ConfirmationEntry[] = useMemo(
    () =>
      stream.state.pendingConfirmations.map((request) => {
        const meta = confirmationMeta[request.request_id];
        const agentId = meta?.agentId ?? null;
        return {
          requestId: request.request_id,
          toolName: request.tool_name,
          toolArguments: request.tool_arguments,
          agentName:
            agentId === null
              ? null
              : agentDisplayName(
                  agentId,
                  templateNames,
                  agents.current.get(agentId)?.name,
                ),
          at: meta?.at ?? new Date().toISOString(),
        };
      }),
    [stream.state.pendingConfirmations, confirmationMeta, templateNames],
  );

  const firstConfirmation = confirmations[0] ?? null;

  /**
   * The run the daemon is blocked on.
   *
   * The frame says so directly — `task_id` — and that is the answer whenever
   * it is there: it is the daemon's own, it cannot be stale, and it is right
   * for a subagent whose `agent_status` has not arrived. The
   * `agent_id → current_task_id` map is the fallback for the frames that carry
   * no run, and `null` rather than a guess when neither answers.
   */
  const blockedRunId = useMemo(() => {
    if (firstConfirmation === null) return null;
    const meta = confirmationMeta[firstConfirmation.requestId];
    if (meta === undefined) return null;
    if (meta.taskId !== null) return meta.taskId;
    if (meta.agentId === null) return null;
    return agents.current.get(meta.agentId)?.taskId ?? null;
  }, [firstConfirmation, confirmationMeta]);

  /**
   * Everything session-local goes when the conversation does.
   *
   * One `useChatSession` instance spans every conversation — `ViewBoundary`
   * resets on the *view*, not the session — so the run reports, artifact and
   * steer cards, resolutions and the finished SSE turn used to follow the user
   * out of one conversation and into the next, where they rendered under a
   * transcript they had nothing to do with. The transient half self-healed on
   * the next send; the cards never did.
   *
   * The trigger is the conversation the daemon *reported* (`session_id` on the
   * history it answered), not the sidebar's pin: an unpinned window is on
   * whatever the lane's active session is, and R48 can replace that under it.
   *
   * A change while a turn is in flight is that turn's own doing — R48 archives
   * and reopens mid-turn — so the id is adopted and nothing is cleared;
   * clearing would close the `EventSource` the user is watching.
   */
  const shownSession = useRef<string | null | undefined>(undefined);
  useEffect(() => {
    if (history.data === undefined) return;
    const reported = history.data.session_id;
    if (shownSession.current === undefined) {
      shownSession.current = reported;
      return;
    }
    if (shownSession.current === reported) return;
    const previous = shownSession.current;
    shownSession.current = reported;
    if (stream.active) return;
    // `null → id` is a lane's first conversation acquiring an identity, not a
    // switch away from one: the turn that just ran is *in* that conversation,
    // and its cards belong under it.
    if (previous === null) return;

    setPending(null);
    setReports([]);
    setArtifacts([]);
    setResolutions([]);
    setSteers([]);
    setConfirmationMeta({});
    started.current.clear();
    stream.reset();
  }, [history.data, stream.active, stream.reset]);

  const items = useMemo(
    () =>
      buildTranscript({
        history: history.data?.messages ?? [],
        reports,
        artifacts,
        confirmations,
        resolutions,
        steers,
        stream: stream.state,
        pending,
        steerLabel: steerRun === null ? undefined : shortTitle(steerRun.title),
      }),
    [
      history.data,
      reports,
      artifacts,
      confirmations,
      resolutions,
      steers,
      stream.state,
      pending,
      steerRun,
    ],
  );

  const send = useCallback(() => {
    const text = draft.trim();
    if (text === "" || sending) return;

    if (steerTargetRunId !== null && composerMode === "queue") {
      // A follow-up is parked on the *lane*, not on the run: the daemon claims
      // it when whatever is running finishes, so — unlike a steer — the target
      // run need not still be listening. `source_task_id` is what records
      // which run it was queued behind.
      if (laneKey === null) {
        // Nothing to park it on: a conversation with no turn yet has no lane.
        showToast(
          "This conversation has no lane yet — send a message first, then queue a follow-up.",
        );
        return;
      }
      const label = steer?.label ?? shortTitle(steerTargetRunId);
      const targetId = steerTargetRunId;
      setSendError(null);
      setSending(true);
      setDraft("");
      void queueFollowup({
        laneKey,
        content: text,
        sourceTaskId: targetId,
        // The picker's project, like a chat turn: the user is queueing work
        // *now*, from here, and that is the project it should re-enter in.
        ...(projectPath === null ? {} : { workspacePath: projectPath }),
      })
        .then(() => {
          setSteers((current) => [
            ...current,
            {
              id: `${targetId}-q${current.length}-${Date.now()}`,
              text,
              mode: "queue",
              label,
              at: new Date().toISOString(),
            },
          ]);
          clearSteerTarget();
        })
        .catch((error: unknown) => {
          // Never silent: each refusal gets its own sentence, and the text goes
          // back in the composer so a rejected queue does not cost the user
          // their message.
          const message = followupErrorMessage(error);
          setSendError(message);
          showToast(message);
          setDraft(text);
        })
        .finally(() => setSending(false));
      return;
    }

    if (steerTargetRunId !== null && composerMode === "steer") {
      // A steer is a control action on a run, not a chat turn: it goes to that
      // run's own route and never down the stream. The row it leaves in the
      // transcript is therefore session-local — the daemon stores no message
      // for it — and it is only added once the queue has actually taken it.
      const label = steer?.label ?? shortTitle(steerTargetRunId);
      const targetId = steerTargetRunId;
      // No `workspace_path`: the daemon defaults it to the *run's* own
      // `workspace_id`, which is the project this message belongs to. The
      // picker says where the user is now, and a steer that outlives its
      // workflow re-enters as an `unprocessed_steering` follow-up — filed
      // under the picker's project, it would land in the wrong one.
      const onScreen = steerRun !== null;
      setSendError(null);
      setSending(true);
      setDraft("");
      void steerTask(targetId, text)
        .then(() => {
          setSteers((current) => [
            ...current,
            {
              id: `${targetId}-${current.length}-${Date.now()}`,
              text,
              mode: "steer",
              label,
              at: new Date().toISOString(),
            },
          ]);
          clearSteerTarget();
        })
        .catch((error: unknown) => {
          // Never silent, and never a shrug: each refusal code has its own
          // sentence, and the text goes back in the composer so a full queue
          // or a finished run does not cost the user their message. Whether
          // the run is still on screen is the client's to know — it decides
          // which of the two `NOT_FOUND` readings is honest.
          const message = steerErrorMessage(error, onScreen);
          setSendError(message);
          showToast(message);
          setDraft(text);
        })
        .finally(() => setSending(false));
      return;
    }

    setSendError(null);
    setSending(true);
    setDraft("");
    setPending({
      text,
      sent: text,
      at: new Date().toISOString(),
      // A chat turn is never a steer any more — that path returned above.
      steer: null,
    });

    void stream
      .send({
        content: text,
        ...(model === null ? {} : { model }),
        ...workspaceOption(projectPath),
        // A resumed conversation is addressed by id, and R49 then makes its
        // project govern this turn instead of the header above. Nothing is
        // sent while the sidebar has nothing pinned, which is what leaves R48
        // free to open a new conversation on a project change.
        ...(selectedSessionId === null ? {} : { sessionId: selectedSessionId }),
      })
      .catch((error: unknown) => {
        // A refused *session* is its own fact — archived elsewhere, or on
        // another lane — and reads nothing like a transport failure.
        setSendError(
          error instanceof ApiError && (error.code ?? "").startsWith("SESSION")
            ? sessionErrorMessage(error)
            : error instanceof Error
              ? error.message
              : "Could not reach the daemon",
        );
        // Put the text back rather than losing it.
        setDraft(text);
        setPending(null);
      })
      .finally(() => setSending(false));
  }, [
    draft,
    sending,
    steerTargetRunId,
    composerMode,
    showToast,
    steer,
    steerRun,
    stream,
    model,
    projectPath,
    selectedSessionId,
    laneKey,
    queueFollowup,
    clearSteerTarget,
  ]);

  /**
   * Take one item back out of the lane's queue.
   *
   * The daemon's cancel is a compare-and-swap against its own autostart, so
   * `409 FOLLOWUP_NOT_QUEUED` is a real answer and not a retryable failure: the
   * item is running now. It gets its own sentence rather than a generic
   * failure toast, and the list refetches either way (`useCancelFollowup`
   * invalidates on settle), so the row leaves the strip in both outcomes.
   */
  const cancelFollowup = useCallback(
    (followupId: number) => {
      if (laneKey === null || cancelPending) return;
      setCancellingFollowupId(followupId);
      cancelFollowupMutation(
        { laneKey, followupId },
        {
          onError: (error: Error) => showToast(followupErrorMessage(error)),
          onSettled: () => setCancellingFollowupId(null),
        },
      );
    },
    [laneKey, cancelPending, cancelFollowupMutation, showToast],
  );

  // The mutation object is a fresh identity every render; holding it in a ref
  // keeps `approve`/`deny` stable, which is what the window key binding wants.
  const respondRef = useRef(stream.respond);
  respondRef.current = stream.respond;

  const answer = useCallback(
    (resolution: Resolution, scope?: ApprovalScope) => {
      const target = firstConfirmation;
      if (target === null) return;
      // When the answer went out — the earliest a `tool_executed` for it can
      // be this one's (G6).
      const answeredAtMs = Date.now();

      respondRef.current.mutate(
        {
          requestId: target.requestId,
          approved: resolution === "approved",
          ...(scope === undefined ? {} : { approvalScope: scope }),
        },
        {
          onSuccess: () => {
            setResolutions((current) => [
              ...current,
              {
                requestId: target.requestId,
                resolution,
                // The tool may already have run and reported: the daemon
                // releases it as soon as the answer lands, and that frame
                // routinely arrives before this callback does.
                note: resolutionNote(
                  resolution,
                  target.toolName,
                  settlingRun(toolRuns.current, target.toolName, answeredAtMs),
                ),
                at: new Date().toISOString(),
              },
            ]);
          },
          onError: (error: Error) => {
            showToast(error.message);
          },
        },
      );
    },
    [firstConfirmation, showToast],
  );

  const approve = useCallback(() => answer("approved"), [answer]);
  const deny = useCallback(() => answer("denied"), [answer]);
  const alwaysAllow = useCallback(() => {
    const toolName = firstConfirmation?.toolName ?? null;
    answer("approved", "entire_tool");
    // §4.4's own copy — the daemon now caches `EntireTool` for the rest of
    // the session, so this is no longer a polite lie.
    if (toolName !== null) {
      showToast(`${toolName} added to the allowlist — it won't ask again`);
    }
  }, [answer, firstConfirmation, showToast]);

  return {
    items,
    // Both calls are the transcript: the probe is the only one on a short
    // conversation, and on a long one the tail is what is rendered.
    historyLoading:
      historyProbe.isLoading || (tailOffset > 0 && historyTail.isLoading),
    historyError: historyProbe.error ?? historyTail.error,
    laneKey,
    sessionId: history.data?.session_id ?? null,

    draft,
    setDraft,
    send,
    sending,
    sendError,

    blocked: stream.blocked,
    pendingToolName: firstConfirmation?.toolName ?? null,
    blockedRunId,
    answering: stream.respond.isPending,
    approve,
    deny,
    alwaysAllow,

    confirmationsError: pendingConfirmations.error,

    activeRuns,
    steer,

    followups: followupQueue.data ?? [],
    followupsError: followupQueue.error,
    cancelFollowup,
    cancellingFollowupId,
  };
}
