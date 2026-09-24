/**
 * The Note tab: the note's Markdown, rendered, and where it came from.
 *
 * **Elements, never HTML.** A note is a file under the owner's store that any
 * editor, script or cloned repository can write, so its text is untrusted:
 * it goes through the transcript's own `MessageBody`, whose parser hands back
 * structure and maps it to elements — a `<script>` or an `<img onerror>` in a
 * note is simply text, and no sanitiser has to be right about it.
 *
 * *Where this came from* is the source evidence (design §7): who wrote it, the
 * DB handles it cites (a run opens in the Work view; a message is named — the
 * transcript has no route to one row), the notes it replaces or that replace
 * it, and its file. It is a claim, not proof (design §4), and says so only by
 * being a list of handles rather than a verdict.
 */

import { MessageBody } from "@/components/chat";
import { Eyebrow } from "@/components/ui";
import type { MemoryNote } from "@/lib/api/memories";
import { useUiStore } from "@/stores/ui";
import { relativeTime } from "@/views/library/format";

import {
  CANDIDATE_NOTE,
  freshnessNote,
  originLabel,
  scopeLabel,
  sourceLabel,
} from "./copy";

export function NoteTab({ note }: { note: MemoryNote }) {
  const focusRun = useUiStore((s) => s.focusRun);
  const openMemory = useUiStore((s) => s.openMemory);
  const freshness = freshnessNote(note.freshness);

  return (
    <div className="max-w-[660px]">
      {note.effective_status === "candidate" && (
        <p className="mt-0 mb-[14px] rounded-2xl border border-amber-line bg-amber-surface px-[12px] py-[9px] text-base leading-[1.5] text-amber-ink">
          {CANDIDATE_NOTE}
        </p>
      )}
      {note.effective_status === "superseded" && (
        <p className="mt-0 mb-[14px] text-base leading-[1.5] text-tertiary">
          A newer note replaces this one, so it is left out of answers.
        </p>
      )}

      {note.body.trim() === "" ? (
        <p className="m-0 text-md text-muted-fg">This note has no text.</p>
      ) : (
        <MessageBody text={note.body} spacing="assistant" />
      )}
      {note.truncated && (
        <p className="mt-[10px] mb-0 font-mono text-2xs-plus text-faint">
          Only the first 32 KiB are shown; the file holds more.
        </p>
      )}

      <section
        aria-label="Where this came from"
        className="mt-[22px] border-t border-line-hair pt-[14px]"
      >
        <Eyebrow className="mb-[8px]">Where this came from</Eyebrow>
        <ul className="m-0 flex list-none flex-col gap-[6px] p-0 text-base leading-[1.5] text-secondary">
          <li>{originLabel(note.origin)}</li>

          {note.sources.length === 0 ? (
            <li className="text-tertiary">It cites no message or run.</li>
          ) : (
            note.sources.map((source) => (
              <li
                key={`${source.type}:${source.id}`}
                className="flex flex-wrap items-baseline gap-[8px]"
              >
                {source.type === "task" ? (
                  <button
                    type="button"
                    onClick={() => focusRun(source.id)}
                    className="cursor-pointer border-none bg-transparent p-0 font-mono text-sm text-blue underline hover:text-blue-hover"
                  >
                    {sourceLabel(source)}
                  </button>
                ) : (
                  <span className="font-mono text-sm">
                    {sourceLabel(source)}
                  </span>
                )}
                {source.observed_at !== null && (
                  <span className="font-mono text-2xs-plus text-faint">
                    seen {relativeTime(source.observed_at)}
                  </span>
                )}
              </li>
            ))
          )}

          {note.sources_status === "source_forgotten" && (
            <li className="text-red-ink">
              Something this note cites was forgotten after it was written, so
              it is left out of answers until you edit it.
            </li>
          )}

          {note.supersedes.length > 0 && (
            <li className="flex flex-wrap items-baseline gap-[8px]">
              <span>Replaces</span>
              {note.supersedes.map((id) => (
                <NoteLink key={id} id={id} onOpen={openMemory} />
              ))}
            </li>
          )}
          {note.superseded_by.length > 0 && (
            <li className="flex flex-wrap items-baseline gap-[8px]">
              <span>Replaced by</span>
              {note.superseded_by.map((id) => (
                <NoteLink key={id} id={id} onOpen={openMemory} />
              ))}
            </li>
          )}

          {note.conflict && (
            <li className="text-red-ink">
              In conflict with another note — two notes claim the same fact, or
              this one names a note it has no authority to replace.{" "}
              <code className="font-mono text-sm">
                openalpaca memory doctor
              </code>{" "}
              lists the pair.
            </li>
          )}
          {freshness !== null && <li className="text-tertiary">{freshness}</li>}

          <li className="font-mono text-2xs-plus text-faint">
            {scopeLabel(note.scope)} · {note.path}
          </li>
        </ul>
      </section>
    </div>
  );
}

function NoteLink({
  id,
  onOpen,
}: {
  id: string;
  onOpen: (id: string) => void;
}) {
  return (
    <button
      type="button"
      onClick={() => onOpen(id)}
      className="cursor-pointer border-none bg-transparent p-0 font-mono text-sm text-blue underline hover:text-blue-hover"
    >
      {id.slice(0, 8)}
    </button>
  );
}
