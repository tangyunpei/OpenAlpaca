/**
 * The Memory view's right column: one note (memory redesign, design §7
 * *Surfaces*, §7a `GET|POST|DELETE /v1/memories/{id}`).
 *
 * The Library detail's shape (§2.4): a pinned head — title, what the note is,
 * who wrote it, the verbs — over a scrolling body with four tabs:
 *
 *   Note     the Markdown rendered **as elements, never HTML** (a note is a
 *            file anyone can edit, so its text is untrusted), and where it
 *            came from: the sources it cites, the notes it replaces or that
 *            replace it, its file, its freshness.
 *   Raw      the Markdown as written, and the editor.
 *   Diff     the current text against one of the kept versions.
 *   History  the kept versions — the last five the service wrote.
 *
 * Every verb sends the revision this window read, in `If-Match`. A note that
 * moved meanwhile is a `409` and is said as such — never retried with the new
 * revision behind the owner's back. A **suggestion** (a candidate naming the
 * note it would replace, design 09 §3.5) is confirmed with *Replace*, which
 * also sends the replaced note's revision as read here, as `target_revision`.
 */

import { useState } from "react";

import { Button, SectionEmpty, Tab, Tag } from "@/components/ui";
import {
  useForgetMemory,
  useMemoryNote,
  useUpdateMemory,
} from "@/hooks/useMemories";
import {
  isMemoryConflict,
  type MemoryNote,
  type MemoryUpdate,
} from "@/lib/api/memories";
import { ApiError } from "@/lib/http";
import { useUiStore, type MemoryTab } from "@/stores/ui";
import { relativeTime } from "@/views/library/format";

import {
  MEMORY_ACTION_CONFLICT,
  originLabel,
  replacedToast,
  scopeLabel,
  statusTag,
} from "./copy";
import { MemoryDiffTab } from "./MemoryDiffTab";
import { MemoryHistoryTab } from "./MemoryHistoryTab";
import { NoteTab } from "./NoteTab";
import { RawTab, type MemoryDraft } from "./RawTab";

const TABS: readonly { id: MemoryTab; label: string }[] = [
  { id: "note", label: "Note" },
  { id: "raw", label: "Raw" },
  { id: "diff", label: "Diff" },
  { id: "history", label: "History" },
];

/** What the last forget in this view left behind, said where the note was. */
export interface ForgottenNotice {
  title: string;
  /** The daemon's own account of what a forget does not erase. */
  retained: string;
}

export interface MemoryDetailProps {
  noteId: string | null;
  workspacePath: string | null;
  /** Shown in place of the empty state right after a forget. */
  forgotten?: ForgottenNotice | null;
  onForgotten?: (notice: ForgottenNotice) => void;
}

function readError(error: Error): string {
  if (error instanceof ApiError && error.isNotFound) {
    return "This note is not in memory any more.";
  }
  return error.message;
}

function updatedAt(note: MemoryNote): string | null {
  const value = note.frontmatter.updated_at;
  return typeof value === "string" ? value : null;
}

export function MemoryDetail({
  noteId,
  workspacePath,
  forgotten = null,
  onForgotten,
}: MemoryDetailProps) {
  const tab = useUiStore((s) => s.memoryTab);
  const setTab = useUiStore((s) => s.setMemoryTab);
  const clearMemory = useUiStore((s) => s.clearMemory);
  const showToast = useUiStore((s) => s.showToast);

  const note = useMemoryNote(noteId, workspacePath);
  // The note a candidate would replace (design 09 §3.5): its title for the
  // copy, its revision for a Replace.
  const replacesId =
    note.data?.status === "candidate"
      ? (note.data.supersedes[0] ?? null)
      : null;
  const target = useMemoryNote(replacesId, workspacePath);
  const update = useUpdateMemory(workspacePath);
  const forget = useForgetMemory(workspacePath);

  const [armed, setArmed] = useState(false);
  // The kept version the Diff tab compares against; `null` = the newest one.
  const [diffBase, setDiffBase] = useState<string | null>(null);
  // The editor's draft, here so switching tabs mid-edit keeps it.
  const [draft, setDraft] = useState<MemoryDraft | null>(null);

  if (noteId === null) {
    return (
      <DetailShell>
        {forgotten === null ? (
          <SectionEmpty padded={false}>
            Select a note to see it here.
          </SectionEmpty>
        ) : (
          <SectionEmpty padded={false} note={forgotten.retained}>
            “{forgotten.title}” is forgotten.
          </SectionEmpty>
        )}
      </DetailShell>
    );
  }

  if (note.error !== null) {
    return (
      <DetailShell>
        <SectionEmpty padded={false} note={note.error.message}>
          {readError(note.error)}
        </SectionEmpty>
      </DetailShell>
    );
  }

  const model = note.data;
  if (model === undefined) {
    return (
      <DetailShell>
        <SectionEmpty padded={false}>Reading the note…</SectionEmpty>
      </DetailShell>
    );
  }

  const busy = update.isPending || forget.isPending;

  const send = (change: MemoryUpdate, done: string, verb: string) => {
    update.mutate(
      { id: model.id, revision: model.revision, update: change },
      {
        onSuccess: () => showToast(done),
        onError: (error) =>
          showToast(
            isMemoryConflict(error)
              ? MEMORY_ACTION_CONFLICT
              : `Could not ${verb} — ${error.message}`,
          ),
      },
    );
  };

  const onForget = () => {
    if (!armed) {
      setArmed(true);
      return;
    }
    setArmed(false);
    forget.mutate(
      { id: model.id, revision: model.revision },
      {
        onSuccess: (result) => {
          onForgotten?.({ title: model.title, retained: result.retained.note });
          clearMemory();
          showToast(`Forgot “${model.title}”`);
        },
        onError: (error) =>
          showToast(
            isMemoryConflict(error)
              ? MEMORY_ACTION_CONFLICT
              : `Could not forget — ${error.message}`,
          ),
      },
    );
  };

  const status = model.effective_status;
  const when = updatedAt(model);
  const tags = [
    statusTag(status),
    model.conflict ? "conflict" : null,
    model.freshness === "verified" ? null : model.freshness,
  ].filter((tag): tag is string => tag !== null);

  return (
    <DetailShell
      head={
        <div>
          <div className="flex items-start gap-[12px]">
            <div className="min-w-0 flex-1">
              <h2 className="m-0 text-2xl font-semibold tracking-tighter text-ink">
                {model.title}
              </h2>
              <div className="mt-[6px] flex flex-wrap items-center gap-[10px] font-mono text-xs text-muted-fg">
                <span>{model.kind}</span>
                <span>{scopeLabel(model.scope)}</span>
                <span>{originLabel(model.origin)}</span>
                {when !== null && <span>{relativeTime(when)}</span>}
                {tags.map((tag) => (
                  <Tag
                    key={tag}
                    value={tag}
                    tone={tag === "conflict" ? "warn" : "neutral"}
                  />
                ))}
              </div>
            </div>

            <div className="flex shrink-0 gap-[6px]">
              {status === "candidate" && replacesId === null && (
                <Button
                  variant="primarySm"
                  disabled={busy}
                  onClick={() =>
                    send(
                      { action: "confirm" },
                      `Confirmed — “${model.title}” is used in answers now`,
                      "confirm",
                    )
                  }
                >
                  Confirm
                </Button>
              )}
              {status === "candidate" && replacesId !== null && (
                <Button
                  variant="primarySm"
                  disabled={busy || target.data === undefined}
                  title={
                    target.error === null
                      ? undefined
                      : `The note this would replace cannot be read: ${target.error.message}`
                  }
                  onClick={() => {
                    if (target.data === undefined) return;
                    send(
                      {
                        action: "confirm",
                        target_revision: target.data.revision,
                      },
                      replacedToast(target.data.title),
                      "replace",
                    );
                  }}
                >
                  Replace
                </Button>
              )}
              {status === "active" && (
                <Button
                  variant="ghostSm"
                  disabled={busy}
                  onClick={() =>
                    send(
                      { action: "archive" },
                      `Archived “${model.title}”`,
                      "archive",
                    )
                  }
                >
                  Archive
                </Button>
              )}
              <Button
                variant={armed ? "dangerGhost" : "ghostSm"}
                disabled={busy}
                onClick={onForget}
                onBlur={() => setArmed(false)}
              >
                {armed ? "Confirm forget" : "Forget"}
              </Button>
            </div>
          </div>

          <div
            role="tablist"
            aria-label="Note views"
            className="mt-[16px] flex gap-[2px] border-b border-line-subtle"
          >
            {TABS.map((entry) => (
              <Tab
                key={entry.id}
                size="library"
                label={entry.label}
                active={tab === entry.id}
                onClick={() => setTab(entry.id)}
              />
            ))}
          </div>
        </div>
      }
    >
      {tab === "note" && (
        <NoteTab note={model} replaces={target.data?.title ?? null} />
      )}
      {tab === "raw" && (
        <RawTab
          note={model}
          draft={draft}
          setDraft={setDraft}
          onReload={() => note.refetch()}
          workspacePath={workspacePath}
        />
      )}
      {tab === "diff" && (
        <MemoryDiffTab
          note={model}
          base={diffBase}
          onBaseChange={setDiffBase}
          workspacePath={workspacePath}
        />
      )}
      {tab === "history" && (
        <MemoryHistoryTab
          note={model}
          workspacePath={workspacePath}
          onCompare={(revision) => {
            setDiffBase(revision);
            setTab("diff");
          }}
        />
      )}
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
