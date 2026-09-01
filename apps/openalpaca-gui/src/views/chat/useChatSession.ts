/**
 * The chat view's glue: history, the live SSE turn, confirmations, run reports.
 *
 * Everything stateful about a chat lane lives here so `ChatView` stays a
 * layout. The streaming machine itself is *not* re-implemented — `useChatStream`
 * owns the SSE contract; this hook only decorates it with the things
 * the design shows around a turn.
 *
 * Honest wiring, gap by gap:
 *   * **GAP-02** steering has no endpoint, so a steered message is sent down
 *     the chat channel with the literal `/steer ` prefix the orchestrator
 *     strips. It targets the lane's active workflow, not a chosen run.
 *   * **GAP-03** queueing a follow-up has no write route at all, so the
 *     composer refuses and says why instead of quietly sending a chat message.
 *   * **GAP-01** `Always allow` sends `approval_scope: "entire_tool"`, which
 *     the daemon drops; the toast says that rather than the design's
 *     "it won't ask again".
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
  type SteerRef,
} from "@/components/chat";
import { toUiStatus, type UiStatus } from "@/components/ui";
import { useChatHistory, useChatStream } from "@/hooks/useChat";
import { useServerEvent } from "@/hooks/useDaemonEvents";
import { useTasks } from "@/hooks/useTasks";
import type { ApprovalScope } from "@/lib/api/types";
import { GAPS, gapNote } from "@/lib/unavailable";
import { useUiStore, type ComposerMode } from "@/stores/ui";

import {
  buildTranscript,
  type ConfirmationEntry,
  type PendingTurn,
  type ResolutionEntry,
  type RunReportData,
  type TranscriptItem,
} from "./transcript-model";

/** Constant identities: `useServerEvent` keys its subscription off the list. */
const RUN_EVENTS = ["workflow_started", "task_status"] as const;
const AGENT_EVENTS = ["agent_status"] as const;
const CONFIRM_EVENTS = ["tool_confirmation_requested"] as const;
const TOOL_EVENTS = ["tool_executed"] as const;

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

  const model = useUiStore((s) => s.model);
  const steerTargetRunId = useUiStore((s) => s.steerTargetRunId);
  const composerMode = useUiStore((s) => s.composerMode);
  const clearSteerTarget = useUiStore((s) => s.clearSteerTarget);
  const showToast = useUiStore((s) => s.showToast);

  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState<PendingTurn | null>(null);
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const [reports, setReports] = useState<RunReportData[]>([]);
  const [resolutions, setResolutions] = useState<ResolutionEntry[]>([]);
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
        confirmations,
        resolutions,
        stream: stream.state,
        pending,
        steerLabel: steerRun === null ? undefined : shortTitle(steerRun.title),
      }),
    [
      history.data,
      reports,
      confirmations,
      resolutions,
      stream.state,
      pending,
      steerRun,
    ],
  );

  const send = useCallback(() => {
    const text = draft.trim();
    if (text === "" || sending) return;

    if (steerTargetRunId !== null && composerMode === "queue") {
      // There is no follow-up write route; sending this as ordinary chat would
      // silently do something else than the user asked for.
      showToast(gapNote(GAPS["GAP-03"]));
      return;
    }

    const steered = steerTargetRunId !== null && composerMode === "steer";
    const sent = steered ? `/steer ${text}` : text;
    const steerRef: SteerRef | null =
      steered && steer !== null ? { mode: "steer", label: steer.label } : null;

    setSendError(null);
    setSending(true);
    setDraft("");
    setPending({
      text,
      sent,
      at: new Date().toISOString(),
      steer: steerRef,
    });

    void stream
      .send({ content: sent, ...(model === null ? {} : { model }) })
      .catch((error: unknown) => {
        setSendError(
          error instanceof Error ? error.message : "Could not reach the daemon",
        );
        // Put the text back rather than losing it.
        setDraft(text);
        setPending(null);
      })
      .finally(() => setSending(false));

    if (steered) clearSteerTarget();
  }, [
    draft,
    sending,
    steerTargetRunId,
    composerMode,
    showToast,
    steer,
    stream,
    model,
    clearSteerTarget,
  ]);

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
    answer("approved", "entire_tool");
    showToast(gapNote(GAPS["GAP-01"]));
  }, [answer, showToast]);

  return {
    items,
    historyLoading: history.isLoading,
    historyError: history.error,
    laneKey: history.data?.lane_key ?? stream.state.laneKey,

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
  };
}
