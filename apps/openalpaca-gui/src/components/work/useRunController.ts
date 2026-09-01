/**
 * The one place a run action turns into an effect.
 *
 * Three of the design's verbs are real HTTP (`POST /v1/tasks/{id}/action`);
 * three are pure navigation; two do not exist and are never reachable because
 * their buttons render disabled (see `run-actions.ts`). The controller still
 * handles them defensively — a disabled button is a UI fact, not a guarantee.
 *
 * Toast copy is §4.4's, with the run's full title in place of the design's
 * hand-written `short` (there is no short title on the wire).
 */

import { useCallback, useState } from "react";

import { ApiError } from "@/lib/http";
import { useTaskAction } from "@/hooks/useTasks";
import type { TaskAction } from "@/lib/api/types";
import { useUiStore } from "@/stores/ui";

import { actionToast, type RunActionId } from "./run-actions";
import type { Run } from "./run-model";

/** The three verbs `apply_task_action` accepts. */
const HTTP_ACTIONS: Partial<Record<RunActionId, TaskAction>> = {
  pause: "pause",
  resume: "resume",
  cancel: "cancel",
};

export interface RunBusy {
  runId: string;
  action: RunActionId;
}

export interface RunController {
  perform: (action: RunActionId, run: Run) => void;
  /** `+ n more in Library ↗` — §4.4's `r.allFiles`. */
  openRunFiles: (run: Run) => void;
  /** The action currently in flight, if any. */
  busy: RunBusy | null;
  /** `busy.action` when `runId` matches — what `RunCard` wants. */
  busyFor: (runId: string) => RunActionId | null;
}

export function useRunController(): RunController {
  const mutation = useTaskAction();
  const setView = useUiStore((s) => s.setView);
  const setSteerTarget = useUiStore((s) => s.setSteerTarget);
  const clearSteerTarget = useUiStore((s) => s.clearSteerTarget);
  const setLibraryKind = useUiStore((s) => s.setLibraryKind);
  const openArtifact = useUiStore((s) => s.openArtifact);
  const showToast = useUiStore((s) => s.showToast);
  const [busy, setBusy] = useState<RunBusy | null>(null);

  const { mutate } = mutation;

  const perform = useCallback(
    (action: RunActionId, run: Run) => {
      const verb = HTTP_ACTIONS[action];
      if (verb !== undefined) {
        setBusy({ runId: run.id, action });
        mutate(
          { id: run.id, action: verb },
          {
            onSuccess: () => {
              const toast = actionToast(action, run.title);
              if (toast !== null) showToast(toast);
            },
            onError: (error: Error) => {
              // A 409 carries the daemon's own sentence ("cannot pause a
              // completed task"); showing it beats inventing one.
              showToast(
                error instanceof ApiError
                  ? error.message
                  : `Could not ${action} ${run.title}`,
              );
            },
            onSettled: () => setBusy(null),
          },
        );
        return;
      }

      switch (action) {
        case "steer":
          setSteerTarget(run.id, "steer");
          return;
        case "queue":
          // Unreachable: the control is disabled (GAP-03). If a follow-up
          // route lands, this becomes `setSteerTarget(run.id, "queue")`.
          return;
        case "jump":
          clearSteerTarget();
          setView("chat");
          return;
        case "start":
        case "rerun":
          // Unreachable: both controls are disabled (GAP-06).
          return;
        default:
          return;
      }
    },
    [clearSteerTarget, mutate, setSteerTarget, setView, showToast],
  );

  const openRunFiles = useCallback(
    (run: Run) => {
      setLibraryKind("All");
      const first = run.artifacts.find((artifact) => artifact.id !== null);
      // Without an artifact id there is nothing to select — the Library opens
      // unfiltered rather than on a guessed row (GAP-04).
      if (first?.id !== undefined && first.id !== null) openArtifact(first.id);
      else setView("library");
    },
    [openArtifact, setLibraryKind, setView],
  );

  const busyFor = useCallback(
    (runId: string) =>
      busy !== null && busy.runId === runId ? busy.action : null,
    [busy],
  );

  return { perform, openRunFiles, busy, busyFor };
}
