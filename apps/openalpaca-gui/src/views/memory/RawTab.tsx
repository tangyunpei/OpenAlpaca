/**
 * The Raw tab: the note's Markdown as written, and the editor.
 *
 * **A draft is never lost to a conflict.** The editor captures the revision it
 * opened at and sends it as `If-Match`; if the note moved meanwhile — another
 * window, the model, an editor on disk — the save is a `409`, and the design's
 * sentence says so over the draft, which stays where it is. `Reload` fetches
 * the current version and shows it above the draft, so the owner can merge by
 * eye, and the next `Save` is sent against the version they have now seen —
 * never against one this window picked up behind their back. A note the
 * window sees move before the save is the same case, said before the save.
 *
 * Only what changed is sent: `patch` fields that are absent are preserved
 * byte for byte (design §7a), so an edit of the body cannot rewrite a title
 * someone else just changed.
 *
 * The draft is held by the detail, not by this tab, so a look at the Note or
 * Diff tab mid-edit does not throw it away.
 */

import type { Dispatch, SetStateAction } from "react";

import { Button } from "@/components/ui";
import { useUpdateMemory } from "@/hooks/useMemories";
import {
  isMemoryConflict,
  type MemoryNote,
  type MemoryPatch,
} from "@/lib/api/memories";
import { useUiStore } from "@/stores/ui";

import { MEMORY_CONFLICT_COPY } from "./copy";

const FIELD =
  "w-full rounded-md border border-line bg-raised px-[8px] py-[5px] text-base text-ink";

export interface MemoryDraft {
  title: string;
  body: string;
  /** The version the draft is an edit of — sent as `If-Match`. */
  base: MemoryNote;
  /** The current version, once the owner reloaded over a conflict. */
  current: MemoryNote | null;
  /** The save came back `409`. */
  refused: boolean;
  error: string | null;
}

export interface RawTabProps {
  note: MemoryNote;
  draft: MemoryDraft | null;
  setDraft: Dispatch<SetStateAction<MemoryDraft | null>>;
  /** Refetch the note; resolves with the fresh copy. */
  onReload: () => Promise<{ data?: MemoryNote }>;
  workspacePath: string | null;
}

/** The fields of `draft` that differ from the version it edits. */
function patchOf(draft: MemoryDraft): MemoryPatch {
  const patch: MemoryPatch = {};
  if (draft.title !== draft.base.title) patch.title = draft.title;
  if (draft.body !== draft.base.body) patch.body = draft.body;
  return patch;
}

export function RawTab({
  note,
  draft,
  setDraft,
  onReload,
  workspacePath,
}: RawTabProps) {
  const showToast = useUiStore((s) => s.showToast);
  const update = useUpdateMemory(workspacePath);

  if (draft === null) {
    return (
      <div className="max-w-[760px]">
        <div className="mb-[10px] flex items-center gap-[10px]">
          <span className="min-w-0 flex-1 truncate font-mono text-2xs-plus text-faint">
            {note.path}
          </span>
          <Button
            variant="ghostSm"
            disabled={note.truncated}
            title={
              note.truncated
                ? "Longer than 32 KiB — edit the file itself, or with `openalpaca memory edit`."
                : undefined
            }
            onClick={() =>
              setDraft({
                title: note.title,
                body: note.body,
                base: note,
                current: null,
                refused: false,
                error: null,
              })
            }
          >
            Edit
          </Button>
        </div>
        <pre
          aria-label="Raw Markdown"
          className="m-0 overflow-x-auto rounded-md bg-muted p-[12px] font-mono text-sm leading-[1.6] whitespace-pre-wrap text-body"
        >
          {note.body}
        </pre>
      </div>
    );
  }

  // The window saw the note move under the draft — the same conflict, said
  // before the save rather than after it.
  const conflict = draft.refused || note.revision !== draft.base.revision;
  const patch = patchOf(draft);
  const unchanged = Object.keys(patch).length === 0;

  const save = () => {
    update.mutate(
      {
        id: draft.base.id,
        revision: draft.base.revision,
        update: { action: "edit", patch },
      },
      {
        onSuccess: () => {
          setDraft(null);
          showToast(`Saved “${draft.title}”`);
        },
        onError: (error) =>
          setDraft((d) =>
            d === null
              ? d
              : isMemoryConflict(error)
                ? { ...d, refused: true, error: null }
                : { ...d, error: error.message },
          ),
      },
    );
  };

  const reload = () => {
    void onReload().then((result) => {
      const fresh = result.data;
      if (fresh === undefined) return;
      setDraft((d) =>
        d === null
          ? d
          : { ...d, base: fresh, current: fresh, refused: false, error: null },
      );
    });
  };

  return (
    <div className="flex max-w-[760px] flex-col gap-[10px]">
      {conflict && (
        <div
          role="alert"
          className="flex flex-wrap items-center gap-[10px] rounded-2xl border border-amber-line bg-amber-surface px-[12px] py-[9px]"
        >
          <span className="min-w-0 flex-1 text-base leading-[1.5] text-amber-ink">
            {MEMORY_CONFLICT_COPY}
          </span>
          <Button variant="outlineRaised" onClick={reload}>
            Reload
          </Button>
        </div>
      )}

      {draft.current !== null && (
        <section
          aria-label="Current version"
          className="rounded-md border border-line-subtle bg-sunken px-[12px] py-[10px]"
        >
          <p className="mt-0 mb-[6px] font-mono text-2xs-plus text-faint uppercase">
            Current version — {draft.current.title}
          </p>
          <pre className="m-0 font-mono text-sm leading-[1.6] whitespace-pre-wrap text-secondary">
            {draft.current.body}
          </pre>
        </section>
      )}

      <input
        aria-label="Title"
        value={draft.title}
        onChange={(event) =>
          setDraft({ ...draft, title: event.target.value, error: null })
        }
        className={FIELD}
      />
      <textarea
        aria-label="Body"
        value={draft.body}
        rows={16}
        onChange={(event) =>
          setDraft({ ...draft, body: event.target.value, error: null })
        }
        className={`${FIELD} resize-y font-mono text-sm leading-[1.6]`}
      />

      {draft.error !== null && (
        <p className="m-0 text-base leading-[1.5] text-red-ink">
          Could not save — {draft.error}
        </p>
      )}

      <div className="flex items-center gap-[8px]">
        <Button
          variant="primarySm"
          disabled={
            unchanged ||
            draft.title.trim() === "" ||
            update.isPending ||
            conflict
          }
          onClick={save}
        >
          Save
        </Button>
        <Button variant="ghostSm" onClick={() => setDraft(null)}>
          Cancel
        </Button>
      </div>
    </div>
  );
}
