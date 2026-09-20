/**
 * The one place a run action turns into an effect.
 *
 * Four of the design's verbs are `POST /v1/tasks/{id}/action`, one is
 * `POST /v1/tasks/{id}/rerun`, and three are pure navigation — `Steer` and
 * `Queue follow-up` aim the composer, `Jump to chat` just switches view.
 * Nothing here is a dead branch any more: GAP-06 closed, so `Start now` and
 * `Re-run` reach the daemon like the rest.
 *
 * The launch verbs render the daemon's refusal rather than a generic failure:
 * a run that did *not* start must never read like one that did, and the codes
 * say which of eight things went wrong (`launchErrorMessage`). `resume` is one
 * of them on an interrupted run — §5.6c's replay — and a plain transition on a
 * paused one, which is why its error path is the launch path and its success
 * copy names the rounds when the daemon sends them.
 *
 * Toast copy is §4.4's, with the run's full title in place of the design's
 * hand-written `short` (there is no short title on the wire).
 */

import { useCallback, useState } from "react";

import { ApiError } from "@/lib/http";
import { useRerunTask, useTaskAction } from "@/hooks/useTasks";
import { launchErrorMessage } from "@/lib/api/tasks";
import type { TaskAction } from "@/lib/api/types";
import { useUiStore } from "@/stores/ui";

import { actionToast, type RunActionId } from "./run-actions";
import type { Run } from "./run-model";

/**
 * The verbs the action route takes. `start` is here with the three
 * transitions because it shares their route and their response — D5 keeps the
 * run's id, so a started run is the row the card is already showing.
 */
const HTTP_ACTIONS: Partial<Record<RunActionId, TaskAction>> = {
  pause: "pause",
  resume: "resume",
  cancel: "cancel",
  start: "start",
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
  const rerun = useRerunTask();
  const setView = useUiStore((s) => s.setView);
  const setSteerTarget = useUiStore((s) => s.setSteerTarget);
  const clearSteerTarget = useUiStore((s) => s.clearSteerTarget);
  const setLibraryKind = useUiStore((s) => s.setLibraryKind);
  const openArtifact = useUiStore((s) => s.openArtifact);
  const showToast = useUiStore((s) => s.showToast);
  const [busy, setBusy] = useState<RunBusy | null>(null);

  const { mutate } = mutation;
  const { mutate: mutateRerun } = rerun;

  const perform = useCallback(
    (action: RunActionId, run: Run) => {
      const verb = HTTP_ACTIONS[action];
      if (verb !== undefined) {
        setBusy({ runId: run.id, action });
        mutate(
          { id: run.id, action: verb },
          {
            onSuccess: (result) => {
              // §5.6c — a replay resume brought a transcript back, and
              // "Audit resumed" reads the same as a run that started over.
              // The clause only appears when the daemon sent the numbers, so
              // an un-pause keeps §4.4's copy exactly.
              const rounds = result.rounds_replayed;
              if (action === "resume" && rounds !== undefined) {
                showToast(
                  `${run.title} resumed — ${rounds} round${rounds === 1 ? "" : "s"} replayed`,
                );
                return;
              }
              const toast = actionToast(action, run.title);
              if (toast !== null) showToast(toast);
            },
            onError: (error: Error) => {
              // `start`'s refusals have their own codes and their own
              // sentences, and so do §5.6c's three for `resume`; a plain
              // transition's 409 carries the daemon's own ("cannot pause a
              // completed task"), which beats inventing one.
              if (action === "start" || action === "resume") {
                showToast(launchErrorMessage(error));
                return;
              }
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
          // Same shape as `steer`: the button aims the composer, and the chat
          // view POSTs to `/v1/lanes/{lane_key}/followups` when the user sends.
          setSteerTarget(run.id, "queue");
          return;
        case "jump":
          clearSteerTarget();
          setView("chat");
          return;
        case "rerun":
          // Its own route, and the only verb that produces a run the user has
          // not seen — so the toast says a *new* one started, rather than
          // reading like this card changed state.
          setBusy({ runId: run.id, action });
          mutateRerun(run.id, {
            onSuccess: (result) => {
              showToast(`${result.title} re-running as a new run`);
            },
            onError: (error: Error) => showToast(launchErrorMessage(error)),
            onSettled: () => setBusy(null),
          });
          return;
        default:
          return;
      }
    },
    [clearSteerTarget, mutate, mutateRerun, setSteerTarget, setView, showToast],
  );

  const openRunFiles = useCallback(
    (run: Run) => {
      setLibraryKind("All");
      const first = run.artifacts.find((artifact) => artifact.id !== null);
      // Without an artifact id there is nothing to select — the Library opens
      // unfiltered rather than on a guessed row.
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
