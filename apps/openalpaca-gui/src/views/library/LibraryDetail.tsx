/**
 * The Library's right column (DESIGN_SPEC §2.4, §3.31, §5.3).
 *
 * Head pinned, body scrolling. Every tab reads from the artifact adapters, and
 * all three of them are unavailable today: the artifact resource itself is
 * GAP-04 and its versions/diff are GAP-05. Where the design would show content,
 * this shows the design's empty voice plus the route the daemon would need —
 * never an invented file.
 *
 * The `available` branches are written against the proposed resources so they
 * light up unchanged the day the routes land. Nothing here draws a diff or a
 * preview of its own: `ArtifactPreview` and `ArtifactDiffTab` are the same
 * components the chat file panel mounts, at `size="full"` instead of
 * `"compact"`, so the two sizes cannot drift apart.
 */

import { SectionEmpty, toFileKind } from "@/components/ui";
import { ArtifactDiffTab } from "@/components/work";
import { ArtifactPreview } from "@/components/work/preview";
import { useDownloadFile, useOpenFile } from "@/hooks/useFiles";
import {
  useArtifact,
  useArtifactDiff,
  useArtifactVersions,
} from "@/hooks/useUnbacked";
import type { Artifact } from "@/lib/api/unbacked";
import { GAPS, gapDetail, gapNote, unavailable } from "@/lib/unavailable";
import { useUiStore } from "@/stores/ui";

import { HistoryTab } from "./HistoryTab";
import { LibraryDetailHeader } from "./LibraryDetailHeader";

/** The artifact resource carries no bytes today, so a preview has no source. */
const CONTENT_NOTE = `${gapNote(GAPS["GAP-04"])} — ${gapDetail(unavailable("GAP-04"))}`;

export interface LibraryDetailProps {
  artifactId: string | null;
}

export function LibraryDetail({ artifactId }: LibraryDetailProps) {
  const tab = useUiStore((s) => s.libraryTab);
  const setTab = useUiStore((s) => s.setLibraryTab);
  const pins = useUiStore((s) => s.pins);
  const togglePin = useUiStore((s) => s.togglePin);
  const showToast = useUiStore((s) => s.showToast);
  const focusRun = useUiStore((s) => s.focusRun);

  const artifact = useArtifact(artifactId);
  const versions = useArtifactVersions(artifactId);
  const diff = useArtifactDiff(artifactId);

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

  if (!artifact.available) {
    return (
      <DetailShell>
        <SectionEmpty padded={false} note={gapDetail(artifact)}>
          This file cannot be opened yet.
        </SectionEmpty>
      </DetailShell>
    );
  }

  const model = artifact.data;
  const pinned = pins[model.id] === true;
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
            const next = togglePin(model.id);
            showToast(`${model.name} ${next ? "pinned" : "unpinned"}`);
          }}
          onExport={onExport}
          onReveal={onReveal}
          onJumpRun={taskId === null ? undefined : () => focusRun(taskId)}
        />
      }
    >
      {tab === "preview" && <PreviewTab artifact={model} />}

      {tab === "diff" && <ArtifactDiffTab diff={diff} size="full" />}

      {tab === "history" &&
        (versions.available ? (
          <HistoryTab versions={versions.data} />
        ) : (
          <SectionEmpty padded={false} note={gapDetail(versions)}>
            No version history for this file.
          </SectionEmpty>
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
 * §3.25, full size — the shared renderer, fed with what the daemon serves.
 *
 * The proposed artifact resource carries a `summary`, never bytes; the content
 * route that would supply them is part of the same gap (GAP-04). Passing the
 * summary through as the document body is honest — it is the artifact's own
 * text — and `null` falls through to the renderer's empty card plus the note.
 */
function PreviewTab({ artifact }: { artifact: Artifact }) {
  const summary =
    artifact.summary === null || artifact.summary.trim() === ""
      ? null
      : artifact.summary;
  return (
    <ArtifactPreview
      size="full"
      meta={{
        name: artifact.name,
        kind: toFileKind(artifact.kind),
        byline:
          artifact.version_count > 1
            ? `v${artifact.version} of ${artifact.version_count}`
            : null,
      }}
      content={summary}
      note={summary === null ? CONTENT_NOTE : null}
    />
  );
}
