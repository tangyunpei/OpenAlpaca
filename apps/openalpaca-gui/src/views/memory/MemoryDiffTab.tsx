/**
 * The Diff tab: the current note against one of its kept versions.
 *
 * The daemon keeps the last five revisions it wrote (design §2) and serves
 * each one's text; the patch is built here (`line-diff.ts`) and drawn by the
 * Library's own `DiffView`, so a note's diff and an artifact's look the same.
 * The newest kept version that is not the current one is compared by default;
 * the picker (and the History tab's `Compare`) chooses another.
 */

import { useMemo } from "react";

import { SectionEmpty } from "@/components/ui";
import { DiffView } from "@/components/work";
import { useMemoryHistory, useMemoryRevision } from "@/hooks/useMemories";
import type { MemoryNote } from "@/lib/api/memories";
import { relativeTime } from "@/views/library/format";

import { shortRevision } from "./copy";
import { unifiedPatch } from "./line-diff";

export interface MemoryDiffTabProps {
  note: MemoryNote;
  /** The kept revision to compare with; `null` = the newest older one. */
  base: string | null;
  onBaseChange: (revision: string) => void;
  workspacePath: string | null;
}

export function MemoryDiffTab({
  note,
  base,
  onBaseChange,
  workspacePath,
}: MemoryDiffTabProps) {
  const history = useMemoryHistory(note.id, workspacePath);
  const older = (history.data ?? []).filter(
    (entry) => entry.revision !== note.revision,
  );
  const chosen =
    older.find((entry) => entry.revision === base) ?? older[0] ?? null;
  const kept = useMemoryRevision(
    note.id,
    chosen?.revision ?? null,
    workspacePath,
  );
  const patch = useMemo(
    () =>
      kept.data === undefined ? null : unifiedPatch(kept.data.body, note.body),
    [kept.data, note.body],
  );

  if (history.error !== null) {
    return (
      <SectionEmpty padded={false} note={history.error.message}>
        The kept versions could not be read.
      </SectionEmpty>
    );
  }
  if (history.data === undefined) {
    return <SectionEmpty padded={false}>Reading the history…</SectionEmpty>;
  }
  if (chosen === null) {
    return (
      <SectionEmpty padded={false}>
        Only one version of this note is kept, so there is nothing to compare it
        with.
      </SectionEmpty>
    );
  }

  return (
    <div className="max-w-[760px]">
      <label className="mb-[12px] flex items-center gap-[8px] font-mono text-xs text-muted-fg">
        Compare with
        <select
          aria-label="Compare with"
          value={chosen.revision}
          onChange={(event) => onBaseChange(event.target.value)}
          className="rounded-md border border-line bg-raised px-[8px] py-[4px] font-mono text-2xs-plus text-ink"
        >
          {older.map((entry) => (
            <option key={entry.revision} value={entry.revision}>
              {shortRevision(entry.revision)}
              {entry.written_at === null
                ? ""
                : ` · ${relativeTime(entry.written_at)}`}
            </option>
          ))}
        </select>
      </label>

      {kept.error !== null ? (
        <SectionEmpty padded={false} note={kept.error.message}>
          That version could not be read.
        </SectionEmpty>
      ) : kept.data === undefined || patch === null ? (
        <SectionEmpty padded={false}>Reading that version…</SectionEmpty>
      ) : (
        <>
          {kept.data.title !== note.title && (
            <p className="mt-0 mb-[10px] text-base leading-[1.5] text-secondary">
              Title: “{kept.data.title}” → “{note.title}”
            </p>
          )}
          {(kept.data.truncated || note.truncated) && (
            <p className="mt-0 mb-[10px] font-mono text-2xs-plus text-faint">
              Longer than 32 KiB: this compares the first 32 KiB of each.
            </p>
          )}
          {patch === "" ? (
            <SectionEmpty padded={false}>
              The text is the same in both versions.
            </SectionEmpty>
          ) : (
            <DiffView
              patch={patch}
              size="full"
              fromLabel={shortRevision(chosen.revision)}
              toLabel="current"
              fromTime={
                chosen.written_at === null
                  ? null
                  : relativeTime(chosen.written_at)
              }
            />
          )}
        </>
      )}
    </div>
  );
}
