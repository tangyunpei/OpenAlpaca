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
 *   * **GAP-23** messages carry no run link, so run reports are session-local:
 *     built from the delegation this client started plus the `task_status`
 *     frames it saw. They do not survive a reload.
 *   * The blocked run is resolved through `agent_status`
 *     (`agent_id → current_task_id`) — the only real mapping available. When
 *     that mapping is unknown it stays `null` rather than being guessed.
 */

import { useCallback, useMemo, useRef, useState } from "react";

import {
  executedResolutionNote,
  formatDurationMs,
  pendingResolutionNote,
  shortTitle,
  type Resolution,
} from "@/components/chat";
import { toUiStatus, type UiStatus } from "@/components/ui";
import { useChatHistory, useChatStream } from "@/hooks/useChat";
import { useServerEvent } from "@/hooks/useDaemonEvents";
import {
  useCancelFollowup,
  useFollowups,
  useQueueFollowup,
} from "@/hooks/useFollowups";
import { useTasks } from "@/hooks/useTasks";
import { followupErrorMessage, type FollowupRecord } from "@/lib/api/followups";
import { steerErrorMessage, steerTask } from "@/lib/api/tasks";
import type { ApprovalScope } from "@/lib/api/types";
import { useProjectStore, workspaceOption } from "@/stores/project";
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

/** Constant identities: `useServerEvent` keys its subscription off the list. */
const RUN_EVENTS = ["workflow_started", "task_status"] as const;
const AGENT_EVENTS = ["agent_status"] as const;
const CONFIRM_EVENTS = ["tool_confirmation_requested"] as const;
const TOOL_EVENTS = ["tool_executed"] as const;
const ARTIFACT_EVENTS = ["artifact_written"] as const;

const HISTORY_LIMIT = 100;

interface StartedRun {
  title: string;
  startedAt: string;
}

interface ConfirmationMeta {
  at: string;
  agentId: string | null;
  agentName: string | null;
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

function isTerminal(
  status: string,
): status is "completed" | "failed" | "cancelled" {
  return (
    status === "completed" || status === "failed" || status === "cancelled"
  );
}

function reportStatus(status: string): RunReportData["status"] {
  if (status === "completed") return "done";
  if (status === "cancelled") return "cancelled";
  return "failed";
}

export function useChatSession(): ChatSession {
  const history = useChatHistory({ limit: HISTORY_LIMIT });
  const stream = useChatStream();
  const activeTasks = useTasks({ status: "active" });

  /** The lane this session is on — `null` until its first turn has a key. */
  const laneKey = history.data?.lane_key ?? stream.state.laneKey;

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

  /** Runs this client started, so a `task_status` frame can be reported. */
  const started = useRef(new Map<string, StartedRun>());
  /** `agent_id → { name, current_task_id }` — the only run mapping on the wire. */
  const agents = useRef(new Map<string, AgentRecord>());

  useServerEvent(AGENT_EVENTS, (event) => {
    if (event.type !== "agent_status") return;
    agents.current.set(event.agent_id, {
      name: event.name,
      taskId: event.current_task_id,
    });
  });

  useServerEvent(RUN_EVENTS, (event) => {
    if (event.type === "workflow_started") {
      started.current.set(event.task_id, {
        title: event.title,
        startedAt: event.ts,
      });
      return;
    }
    if (event.type !== "task_status") return;
    if (!isTerminal(event.status)) return;

    const origin = started.current.get(event.task_id);
    // Only report workflows this lane actually started (GAP-23: nothing links
    // a stored message to a run, so a foreign run has no place in this lane).
    if (origin === undefined) return;
    started.current.delete(event.task_id);

    const title = event.title !== "" ? event.title : origin.title;
    setReports((current) =>
      current.some((report) => report.taskId === event.task_id)
        ? current
        : [
            ...current,
            {
              taskId: event.task_id,
              title: title === "" ? "Background workflow" : title,
              status: reportStatus(event.status),
              startedAt: origin.startedAt,
              endedAt: event.ts,
              summary: event.outcome_summary ?? event.result_summary,
              artifactCount: event.artifact_count ?? 0,
            },
          ],
    );
  });

  /**
   * A file an agent wrote, shown inline. Two frames belong to this lane: a
   * loose artifact (no run — the main loop wrote it for this conversation) and
   * one from a workflow this lane started. A foreign run's file belongs to that
   * run's lane, and lands in the Library either way.
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
    const agent = agents.current.get(event.agent_id);
    setConfirmationMeta((current) =>
      current[event.request_id] !== undefined
        ? current
        : {
            ...current,
            [event.request_id]: {
              at: event.ts,
              agentId: event.agent_id,
              agentName: agent?.name ?? event.agent_id,
            },
          },
    );
  });

  useServerEvent(TOOL_EVENTS, (event) => {
    if (event.type !== "tool_executed") return;
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
        return {
          requestId: request.request_id,
          toolName: request.tool_name,
          toolArguments: request.tool_arguments,
          agentName: meta?.agentName ?? null,
          at: meta?.at ?? new Date().toISOString(),
        };
      }),
    [stream.state.pendingConfirmations, confirmationMeta],
  );

  const firstConfirmation = confirmations[0] ?? null;

  const blockedRunId = useMemo(() => {
    if (firstConfirmation === null) return null;
    const meta = confirmationMeta[firstConfirmation.requestId];
    if (meta?.agentId == null) return null;
    return agents.current.get(meta.agentId)?.taskId ?? null;
  }, [firstConfirmation, confirmationMeta]);

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
      })
      .catch((error: unknown) => {
        setSendError(
          error instanceof Error ? error.message : "Could not reach the daemon",
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
                note: pendingResolutionNote(resolution, target.toolName),
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
    historyLoading: history.isLoading,
    historyError: history.error,
    laneKey,

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

    activeRuns,
    steer,

    followups: followupQueue.data ?? [],
    followupsError: followupQueue.error,
    cancelFollowup,
    cancellingFollowupId,
  };
}
