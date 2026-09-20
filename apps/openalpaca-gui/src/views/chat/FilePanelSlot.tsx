/**
 * The aside's file-panel mode (DESIGN_SPEC §3.23), wired to the artifact API.
 *
 * `panelArtifactId` is an artifact id — the same identity the Library uses, and
 * the same one an attachment carries, because uploads and produced files are
 * one resource. So this panel is the Library detail at `size="compact"`: the
 * row from `GET /v1/artifacts/{id}`, the bytes, the versions and the patch,
 * through the very same renderers, with `previewPlan` making the same call
 * about what may be drawn.
 *
 * `Library ↗` carries the current tab across (§4.2 `openInLibrary`).
 */

import { FilePanel, formatClock } from "@/components/chat";
import { toFileKind } from "@/components/ui";
import { ArtifactDiffTab } from "@/components/work";
import { ArtifactPreview } from "@/components/work/preview";
import {
  useArtifact,
  useArtifactDiff,
  useArtifactText,
  useArtifactVersions,
  useArtifacts,
  useTogglePin,
} from "@/hooks/useArtifacts";
import { artifactContentUrl } from "@/lib/api/artifacts";
import { ApiError } from "@/lib/http";
import { useUiStore } from "@/stores/ui";
import { HistoryTab } from "@/views/library/HistoryTab";
import { diffPair, previewPlan } from "@/views/library/preview";

import { fileLanguage } from "./artifact";

/** As many rows as the design's 340px-tall dropdown can usefully show. */
const PICKER_LIMIT = 30;

export interface FilePanelSlotProps {
  artifactId: string;
}

export function FilePanelSlot({ artifactId }: FilePanelSlotProps) {
  const row = useArtifact(artifactId);
  const model = row.data ?? null;

  // The switcher lists the Library — the same rows the Library view shows,
  // capped to what the dropdown can hold.
  const library = useArtifacts({ limit: PICKER_LIMIT });

  const versions = useArtifactVersions(artifactId);
  const pair = model === null ? null : diffPair(model);
  const diff = useArtifactDiff(artifactId, pair?.from ?? 0, pair?.to ?? 0);

  const plan = model === null ? null : previewPlan(model);
  const content = useArtifactText(artifactId, plan?.mode === "text");

  const panelTab = useUiStore((s) => s.panelTab);
  const setPanelTab = useUiStore((s) => s.setPanelTab);
  const pickerOpen = useUiStore((s) => s.pickerOpen);
  const togglePicker = useUiStore((s) => s.togglePicker);
  const closePicker = useUiStore((s) => s.closePicker);
  const pickPanelArtifact = useUiStore((s) => s.pickPanelArtifact);
  const backToWork = useUiStore((s) => s.backToWork);
  const closePanel = useUiStore((s) => s.closePanel);
  const openInLibrary = useUiStore((s) => s.openInLibrary);
  const focusRun = useUiStore((s) => s.focusRun);
  const togglePin = useTogglePin();
  const pins = useUiStore((s) => s.pins);
  const pinned =
    model === null
      ? pins[artifactId] === true
      : (pins[model.id] ?? model.pinned);

  const pickerItems = (library.data?.artifacts ?? []).map((entry) => ({
    id: entry.id,
    name: entry.name,
    kind: toFileKind(entry.kind),
    language: fileLanguage(entry.name),
    pinned: pins[entry.id] ?? entry.pinned,
    stamp: formatClock(entry.updated_at),
  }));

  const artifact =
    model === null
      ? null
      : {
          id: model.id,
          name: model.name,
          kind: toFileKind(model.kind),
          language: fileLanguage(model.name),
          version: model.version,
          agent: model.agent_template_id ?? model.agent_id,
          runId: model.task_title ?? model.task_id,
        };

  const artifactNote =
    artifact !== null
      ? null
      : row.isLoading
        ? "Reading the file…"
        : row.error instanceof ApiError && row.error.isNotFound
          ? "This file is not in the library any more."
          : (row.error?.message ?? "This file could not be read.");

  return (
    <FilePanel
      artifact={artifact}
      artifactNote={artifactNote}
      tab={panelTab}
      onTabChange={setPanelTab}
      pickerOpen={pickerOpen}
      onTogglePicker={togglePicker}
      onClosePicker={closePicker}
      pickerItems={pickerItems}
      pickerNote={
        library.error !== null
          ? `The library could not be listed — ${library.error.message}`
          : library.isLoading
            ? "Reading the library…"
            : pickerItems.length === 0
              ? "Nothing in the library yet."
              : null
      }
      onPickArtifact={pickPanelArtifact}
      onBackToWork={backToWork}
      onClose={closePanel}
      onOpenInLibrary={openInLibrary}
      onJumpRun={
        model?.task_id == null
          ? undefined
          : () => focusRun(model.task_id as string)
      }
      pinned={pinned}
      onTogglePin={() => togglePin.mutate({ id: artifactId, pinned: !pinned })}
      preview={
        model === null || plan === null ? undefined : plan.mode === "image" ? (
          <ArtifactPreview
            size="compact"
            meta={{ name: model.name, kind: "image" }}
            content={null}
            src={artifactContentUrl(model.id)}
            note={null}
          />
        ) : (
          <ArtifactPreview
            size="compact"
            meta={{
              name: model.name,
              kind: plan.mode === "text" ? plan.kind : "term",
              language: fileLanguage(model.name),
            }}
            content={plan.mode === "text" ? (content.data ?? null) : null}
            note={
              plan.mode === "none"
                ? plan.note
                : content.error !== null
                  ? content.error.message
                  : content.isLoading
                    ? "Reading the file…"
                    : plan.note
            }
          />
        )
      }
      diff={
        model === null ? undefined : (
          <ArtifactDiffTab
            size="compact"
            diff={diff.data ?? null}
            note={
              pair === null
                ? "Only one version of this file exists."
                : diff.error !== null
                  ? diff.error.message
                  : diff.isLoading
                    ? "Reading the patch…"
                    : null
            }
          />
        )
      }
      history={
        versions.data === undefined ||
        versions.data.length === 0 ? undefined : (
          <HistoryTab versions={versions.data} size="compact" />
        )
      }
      historyNote={
        versions.error !== null
          ? versions.error.message
          : versions.isLoading
            ? "Reading the history…"
            : "No version history for this file."
      }
    />
  );
}
