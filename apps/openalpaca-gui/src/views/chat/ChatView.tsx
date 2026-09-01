/**
 * The chat view (DESIGN_SPEC §2.2, §5.1).
 *
 * Layout, exactly as §2.2 draws it: a 46px header, a scrolling transcript
 * whose inner column is 720px (780 when dense), the composer, and — only when
 * `workOpen || panelArt` — a 7px resizer and the aside.
 *
 * The aside is **one slot with two modes** (§8.4): the Work pane when
 * `panelArt === null`, the file panel otherwise. They are never both rendered.
 * `‹ Work` restores the pane, `›` collapses the aside, and with the aside
 * fully closed the header grows the design's own re-entry path, the
 * `RunningNowPill`.
 *
 * Density affects exactly three things (§8.3); two of them are here — the
 * transcript's max width and the message gap. The third lives in the run card.
 */

import { useEffect, useRef } from "react";

import {
  Composer,
  DensityToggle,
  RunningNowPill,
  formatHeaderDate,
} from "@/components/chat";
import { Resizer } from "@/components/shell";
import { PaneHeader } from "@/components/ui";
import { useModels } from "@/hooks/useSettings";
import {
  MODEL_SCOPE_NOTE,
  useOrchestratorConfig,
  useUpdateOrchestratorConfig,
} from "@/hooks/useOrchestrator";
import { formatSpend, useTodaySpend } from "@/hooks/useUsage";
import { usePublishConfirmation } from "@/stores/confirmation";
import {
  selectPanelOn,
  selectShowAside,
  selectTranscriptMaxWidth,
  selectWorkClosed,
  useUiStore,
} from "@/stores/ui";

import { FilePanelSlot } from "./FilePanelSlot";
import { Transcript } from "./Transcript";
import { useChatSession } from "./useChatSession";
import { renderDefaultWorkPane, type WorkPaneRenderer } from "./WorkPaneSlot";

export interface ChatViewProps {
  /**
   * Overrides §3.18's `WorkPane` in the aside's work mode. Left unset, the
   * real pane renders — see `WorkPaneSlot`.
   */
  renderWorkPane?: WorkPaneRenderer;
}

export default function ChatView({
  renderWorkPane = renderDefaultWorkPane,
}: ChatViewProps = {}) {
  const dense = useUiStore((s) => s.dense);
  const toggleDense = useUiStore((s) => s.toggleDense);
  const showAside = useUiStore(selectShowAside);
  const transcriptMaxWidth = useUiStore(selectTranscriptMaxWidth);
  const panelOn = useUiStore(selectPanelOn);
  const workClosed = useUiStore(selectWorkClosed);
  const panelArtifactId = useUiStore((s) => s.panelArtifactId);
  const asideWidth = useUiStore((s) => s.paneWidths.workW);
  const openWorkPane = useUiStore((s) => s.openWorkPane);
  const closeWorkPane = useUiStore((s) => s.closeWorkPane);
  const setView = useUiStore((s) => s.setView);
  const setSettingsSection = useUiStore((s) => s.setSettingsSection);
  const clearSteerTarget = useUiStore((s) => s.clearSteerTarget);
  const showToast = useUiStore((s) => s.showToast);

  const model = useUiStore((s) => s.model);
  const setModel = useUiStore((s) => s.setModel);
  const modelPickerOpen = useUiStore((s) => s.modelPickerOpen);
  const toggleModelPicker = useUiStore((s) => s.toggleModelPicker);
  const closeModelPicker = useUiStore((s) => s.closeModelPicker);

  const models = useModels();
  const orchestrator = useOrchestratorConfig();
  const updateOrchestrator = useUpdateOrchestratorConfig();
  const spend = useTodaySpend();

  const session = useChatSession();

  // The app root owns the key ladder (§4.5), the rail's blocked lane bar and
  // the palette's `Approve` row; all three read this one published slot.
  usePublishConfirmation(
    session.blocked && session.pendingToolName !== null
      ? {
          toolName: session.pendingToolName,
          runId: session.blockedRunId,
          approve: session.approve,
          deny: session.deny,
        }
      : null,
  );

  // The store holds no default model on purpose — the daemon's own default is
  // the only truthful starting value (§4.2 seeds a literal; this does not).
  const daemonModel = orchestrator.data?.model ?? null;
  useEffect(() => {
    if (model === null && daemonModel !== null) setModel(daemonModel);
  }, [model, daemonModel, setModel]);

  // Follow the transcript: a new row, or another delta on the live row.
  const scroller = useRef<HTMLDivElement>(null);
  const itemCount = session.items.length;
  const lastItem = session.items.at(-1);
  const liveLength = lastItem?.kind === "assistant" ? lastItem.text.length : 0;
  useEffect(() => {
    const node = scroller.current;
    if (node === null) return;
    node.scrollTop = node.scrollHeight;
  }, [itemCount, liveLength]);

  const activeCount = session.activeRuns.length;

  return (
    <>
      <section className="flex min-w-0 flex-1 flex-col bg-main">
        <PaneHeader title="Chat" variant="chat" meta={formatHeaderDate()}>
          {workClosed && activeCount > 0 && (
            <RunningNowPill count={activeCount} onOpen={openWorkPane} />
          )}
          <DensityToggle dense={dense} onToggle={toggleDense} />
        </PaneHeader>

        <div
          ref={scroller}
          className="sc min-h-0 flex-1 overflow-y-auto pt-[30px] pb-[10px]"
        >
          <div
            className="mx-auto px-[26px]"
            style={{ maxWidth: transcriptMaxWidth }}
          >
            {session.historyError !== null && (
              <p className="mb-[20px] font-mono text-2xs-plus text-faint">
                Could not load this lane: {session.historyError.message}
              </p>
            )}
            <Transcript items={session.items} dense={dense} />
            {session.sendError !== null && (
              <p
                role="alert"
                className="mb-[20px] font-mono text-2xs-plus text-red-ink"
              >
                {session.sendError}
              </p>
            )}
          </div>
        </div>

        <Composer
          blocked={session.blocked}
          pendingToolName={session.pendingToolName ?? undefined}
          answering={session.answering}
          onApprove={session.approve}
          onDeny={session.deny}
          onAlwaysAllow={session.alwaysAllow}
          value={session.draft}
          onChange={session.setDraft}
          onSend={session.send}
          sending={session.sending}
          steer={session.steer}
          onClearSteer={clearSteerTarget}
          models={models.data ?? []}
          model={model}
          modelStatus={
            models.isLoading ? "loading" : models.isError ? "error" : "ready"
          }
          modelPickerOpen={modelPickerOpen}
          onToggleModelPicker={toggleModelPicker}
          onCloseModelPicker={closeModelPicker}
          onPickModel={(modelId) => {
            setModel(modelId);
            updateOrchestrator.mutate({
              model: modelId,
              fallback_models: orchestrator.data?.fallback_models ?? [],
            });
            const provider =
              models.data?.find((entry) => entry.id === modelId)?.provider ??
              "unknown provider";
            showToast(`Chat model → ${modelId} (${provider})`);
          }}
          onManageProviders={() => {
            closeModelPicker();
            setSettingsSection("models");
            setView("settings");
          }}
          modelNote={MODEL_SCOPE_NOTE}
          spend={
            spend.data === undefined ? null : formatSpend(spend.data.costUsd)
          }
        />
      </section>

      {showAside && (
        <Resizer paneKey="workW" direction={-1} label="chat side pane" />
      )}

      {showAside && (
        <aside
          aria-label={panelOn ? "File panel" : "Work pane"}
          style={{ width: asideWidth }}
          className="flex shrink-0 flex-col border-l border-line-strong bg-canvas"
        >
          {panelOn && panelArtifactId !== null ? (
            <FilePanelSlot artifactId={panelArtifactId} />
          ) : (
            renderWorkPane({
              blocked: session.blocked,
              blockedRunId: session.blockedRunId,
              onFullView: () => setView("work"),
              onCollapse: closeWorkPane,
            })
          )}
        </aside>
      )}
    </>
  );
}
