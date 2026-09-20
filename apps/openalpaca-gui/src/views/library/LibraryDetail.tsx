/**
 * The Library's right column (DESIGN_SPEC §2.4, §3.31, §5.3).
 *
 * Head pinned, body scrolling. Every tab reads a real route now: the row from
 * `GET /v1/artifacts/{id}`, the bytes from `…/content`, the versions from
 * `…/versions` and the patch from `…/diff`. Each failure states itself — a
 * vanished file, a 404, a 409 the daemon named — because a blank pane would
 * claim "nothing here", which is a different thing from "this could not be
 * read".
 *
 * Nothing here draws a diff or a preview of its own: `ArtifactPreview` and
 * `ArtifactDiffTab` are the same components the chat file panel mounts, at
 * `size="full"` instead of `"compact"`, so the two sizes cannot drift apart.
 */

import { SectionEmpty, languageFromName } from "@/components/ui";
import { ArtifactDiffTab } from "@/components/work";
import { ArtifactPreview } from "@/components/work/preview";
import {
  useArtifact,
  useArtifactDiff,
  useArtifactText,
  useArtifactVersions,
  useTogglePin,
} from "@/hooks/useArtifacts";
import { useDownloadFile, useOpenFile } from "@/hooks/useFiles";
import { artifactContentUrl, type Artifact } from "@/lib/api/artifacts";
import { ApiError } from "@/lib/http";
import { useUiStore } from "@/stores/ui";

import { HistoryTab } from "./HistoryTab";
import { LibraryDetailHeader } from "./LibraryDetailHeader";
import { diffPair, previewPlan } from "./preview";

export interface LibraryDetailProps {
  artifactId: string | null;
}

/** What went wrong reading the row, in the user's terms rather than HTTP's. */
function detailError(error: Error): string {
  if (error instanceof ApiError && error.isNotFound) {
    return "This file is not in the library any more.";
  }
  return error.message;
}

export function LibraryDetail({ artifactId }: LibraryDetailProps) {
  const tab = useUiStore((s) => s.libraryTab);
  const setTab = useUiStore((s) => s.setLibraryTab);
  const pins = useUiStore((s) => s.pins);
  const showToast = useUiStore((s) => s.showToast);
  const focusRun = useUiStore((s) => s.focusRun);
  const togglePin = useTogglePin();

  const artifact = useArtifact(artifactId);
  const model = artifact.data ?? null;

  const versions = useArtifactVersions(artifactId);
  const pair = model === null ? null : diffPair(model);
  const diff = useArtifactDiff(artifactId, pair?.from ?? 0, pair?.to ?? 0);

  const download = useDownloadFile();
  const open = useOpenFile();

  if (artifactId === null) {
    return (
      <DetailShell>
        <SectionEmpty padded={false}>
          Select a file to see it here.
        </SectionEmpty>
      </DetailShell>
    );
  }

  if (artifact.error !== null) {
    return (
      <DetailShell>
        <SectionEmpty padded={false} note={artifact.error.message}>
          {detailError(artifact.error)}
        </SectionEmpty>
      </DetailShell>
    );
  }

  if (model === null) {
    return (
      <DetailShell>
        <SectionEmpty padded={false}>Reading the file…</SectionEmpty>
      </DetailShell>
    );
  }

  const pinned = pins[model.id] ?? model.pinned;
  // Narrowed once so the jump handler does not need a cast.
  const taskId = model.task_id;

  const onExport = () => {
    download.mutate(model.id, {
      onSuccess: (blob) => {
        const url = URL.createObjectURL(blob);
        const anchor = document.createElement("a");
        anchor.href = url;
        anchor.download = model.name;
        anchor.click();
        URL.revokeObjectURL(url);
        showToast(`${model.name} exported`);
      },
      onError: (error) => showToast(`Export failed — ${error.message}`),
    });
  };

  // The route opens the file with the daemon host's default app; there is no
  // reveal-in-Finder command, so the toast says what actually happened.
  const onReveal = () => {
    open.mutate(model.id, {
      onSuccess: () => showToast(`${model.name} opened in its default app`),
      onError: (error) => showToast(`Could not open — ${error.message}`),
    });
  };

  return (
    <DetailShell
      head={
        <LibraryDetailHeader
          artifact={model}
          pinned={pinned}
          tab={tab}
          onTabChange={setTab}
          onTogglePin={() => {
            togglePin.mutate(
              { id: model.id, pinned: !pinned },
              {
                onSuccess: (state) =>
                  showToast(
                    `${model.name} ${state.pinned ? "pinned" : "unpinned"}`,
                  ),
                onError: (error) =>
                  showToast(`Could not pin — ${error.message}`),
              },
            );
          }}
          onExport={onExport}
          onReveal={onReveal}
          onJumpRun={taskId === null ? undefined : () => focusRun(taskId)}
        />
      }
    >
      {tab === "preview" && <PreviewTab artifact={model} />}

      {tab === "diff" && (
        <ArtifactDiffTab
          size="full"
          diff={diff.data ?? null}
          note={
            pair === null
              ? "Only one version of this file exists, so there is nothing to compare it with."
              : diff.error !== null
                ? diff.error.message
                : diff.isLoading
                  ? "Reading the patch…"
                  : null
          }
        />
      )}

      {tab === "history" &&
        (versions.error !== null ? (
          <SectionEmpty padded={false} note={versions.error.message}>
            The version history could not be read.
          </SectionEmpty>
        ) : versions.data === undefined ? (
          <SectionEmpty padded={false}>Reading the history…</SectionEmpty>
        ) : versions.data.length === 0 ? (
          <SectionEmpty padded={false}>
            No version history for this file.
          </SectionEmpty>
        ) : (
          <HistoryTab versions={versions.data} />
        ))}
    </DetailShell>
  );
}

/** §2.4: the head is pinned and only the body scrolls. */
function DetailShell({
  head,
  children,
}: {
  head?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="flex min-w-0 flex-1 flex-col">
      {head !== undefined && (
        <div className="shrink-0 px-[24px] pt-[16px]">{head}</div>
      )}
      <div className="sc min-h-0 flex-1 overflow-y-auto px-[24px] pt-[20px] pb-[28px]">
        {children}
      </div>
    </section>
  );
}

/**
 * §3.25, full size — the shared renderer, fed with the artifact's own bytes.
 *
 * `previewPlan` decides what may be drawn (and says why when nothing may be);
 * this only fetches when that decision asks for characters.
 */
function PreviewTab({ artifact }: { artifact: Artifact }) {
  const plan = previewPlan(artifact);
  const content = useArtifactText(artifact.id, plan.mode === "text");

  const byline =
    artifact.version_count > 1
      ? `v${artifact.version} of ${artifact.version_count}`
      : null;

  if (plan.mode === "image") {
    return (
      <ArtifactPreview
        size="full"
        meta={{ name: artifact.name, kind: "image", byline }}
        content={null}
        src={artifactContentUrl(artifact.id)}
        note={
          artifactContentUrl(artifact.id) === null
            ? "Not connected to the daemon, so the image cannot be loaded."
            : null
        }
      />
    );
  }

  const body = plan.mode === "text" ? (content.data ?? null) : null;
  const note =
    plan.mode === "none"
      ? plan.note
      : content.error !== null
        ? content.error.message
        : content.isLoading
          ? "Reading the file…"
          : plan.note;

  return (
    <>
      {/* The renderer only shows a note in place of a document, so a note that
          accompanies one (markup shown as source) is drawn above it. */}
      {body !== null && note !== null && (
        <p className="mt-0 mb-[10px] font-mono text-2xs-plus text-faint">
          {note}
        </p>
      )}
      <ArtifactPreview
        size="full"
        meta={{
          name: artifact.name,
          kind: plan.mode === "text" ? plan.kind : "term",
          byline,
          language: languageFromName(artifact.name),
        }}
        content={body}
        note={note}
      />
    </>
  );
}
