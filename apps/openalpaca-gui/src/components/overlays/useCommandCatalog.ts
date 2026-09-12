/**
 * The live ⌘K catalogue (DESIGN_SPEC §3.33, §4.4).
 *
 * `buildCommands` is pure; this is the one place that binds it to the app, so
 * the palette's rows and the globally bound shortcuts are the *same* list. Two
 * separate assemblies would let a shortcut exist with no row — or, worse, let
 * two `window` listeners run one command twice.
 *
 * `pendingConfirmation` defaults to the chat lane's published confirmation.
 * Passing it explicitly overrides that — the palette's tests do, and so could a
 * host that owns the confirmation itself.
 */

import { useMemo } from "react";

import { useTasks } from "@/hooks/useTasks";
import { useConfirmationStore } from "@/stores/confirmation";
import { useUiStore } from "@/stores/ui";

import { buildCommands, type Command } from "./commands";

/** What the `Approve` row needs; the rest of the confirmation is irrelevant. */
export interface PendingConfirmation {
  toolName: string;
  onApprove: () => void;
}

export function useCommandCatalog(
  pendingConfirmation?: PendingConfirmation | null,
): Command[] {
  const setPaletteOpen = useUiStore((s) => s.setPaletteOpen);
  const setView = useUiStore((s) => s.setView);
  const setSettingsSection = useUiStore((s) => s.setSettingsSection);
  const setSteerTarget = useUiStore((s) => s.setSteerTarget);
  const toggleDense = useUiStore((s) => s.toggleDense);

  // `status: "active"` is the daemon's `queued|running|paused` list mode — the
  // same set the rail calls "Running now". The first row is the steer target.
  const activeTasks = useTasks({ status: "active" });
  const firstActive = activeTasks.data?.[0];
  const activeRunId = firstActive?.id ?? null;
  const activeRunTitle = firstActive?.title ?? null;

  const published = useConfirmationStore((s) => s.pending);
  const confirmation =
    pendingConfirmation !== undefined
      ? pendingConfirmation
      : published === null
        ? null
        : { toolName: published.toolName, onApprove: published.approve };

  const toolName = confirmation?.toolName;
  const onApprove = confirmation?.onApprove;

  return useMemo(
    () =>
      buildCommands({
        activeRun:
          activeRunId === null || activeRunTitle === null
            ? null
            : { id: activeRunId, title: activeRunTitle },
        pendingConfirmation:
          toolName === undefined || onApprove === undefined
            ? null
            : {
                toolName,
                approve: () => {
                  setPaletteOpen(false);
                  onApprove();
                },
              },
        goChat: () => {
          setPaletteOpen(false);
          setView("chat");
        },
        goWork: () => {
          setPaletteOpen(false);
          setView("work");
        },
        goLibrary: () => {
          setPaletteOpen(false);
          setView("library");
        },
        goSettingsSection: (sectionId) => {
          setPaletteOpen(false);
          setView("settings");
          setSettingsSection(sectionId);
        },
        steerRun: (runId) => {
          setPaletteOpen(false);
          setSteerTarget(runId, "steer");
        },
        toggleDense: () => {
          setPaletteOpen(false);
          toggleDense();
        },
      }),
    [
      activeRunId,
      activeRunTitle,
      onApprove,
      setPaletteOpen,
      setSettingsSection,
      setSteerTarget,
      setView,
      toggleDense,
      toolName,
    ],
  );
}
