/**
 * The History tab: the versions of this note the service kept (design §2 —
 * the last five it wrote, the current one included, newest first).
 *
 * The rows are the Library's `VersionRow` treatment — the newest raised — and
 * each older one offers `Compare`, which opens the Diff tab on it. A version
 * written outside the app is not in this list, and the footnote says so,
 * because a reader who edited the file by hand will look for it here.
 */

import { Button, SectionEmpty } from "@/components/ui";
import { useMemoryHistory } from "@/hooks/useMemories";
import type { MemoryNote } from "@/lib/api/memories";
import { cn } from "@/lib/cn";
import { relativeTime } from "@/views/library/format";

import { HISTORY_NOTE, shortRevision } from "./copy";

export interface MemoryHistoryTabProps {
  note: MemoryNote;
  workspacePath: string | null;
  onCompare: (revision: string) => void;
}

export function MemoryHistoryTab({
  note,
  workspacePath,
  onCompare,
}: MemoryHistoryTabProps) {
  const history = useMemoryHistory(note.id, workspacePath);

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

  return (
    <div className="max-w-[660px]">
      <ul
        aria-label="Kept versions"
        className="m-0 flex list-none flex-col gap-[8px] p-0"
      >
        {history.data.map((entry, index) => {
          const current = entry.revision === note.revision;
          return (
            <li
              key={entry.revision}
              className={cn(
                "flex items-center gap-[12px] rounded-2xl border px-[14px] py-[12px]",
                index === 0
                  ? "border-line-popover bg-raised"
                  : "border-line-subtle bg-inactive",
              )}
            >
              <span className="font-mono text-sm font-medium text-ink">
                {shortRevision(entry.revision)}
              </span>
              <span className="min-w-0 flex-1 font-mono text-2xs-plus text-muted-fg">
                {current ? "current" : "kept"}
              </span>
              {entry.written_at !== null && (
                <span className="shrink-0 font-mono text-xs text-muted-fg">
                  {relativeTime(entry.written_at)}
                </span>
              )}
              {!current && (
                <Button
                  variant="ghostXs"
                  aria-label={`Compare ${shortRevision(entry.revision)} with the current version`}
                  onClick={() => onCompare(entry.revision)}
                >
                  Compare
                </Button>
              )}
            </li>
          );
        })}
      </ul>
      <p className="mt-[12px] mb-0 font-mono text-2xs-plus leading-[1.6] text-faint">
        {HISTORY_NOTE}
      </p>
    </div>
  );
}
