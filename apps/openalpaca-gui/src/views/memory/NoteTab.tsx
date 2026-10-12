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
 *
 * **A candidate's text** is what the owner confirms or keeps out, and the
 * model wrote it: every hidden character in it — and in the replaced note's
 * title the copy quotes — is shown as `<U+XXXX>` (`lib/hidden-chars`, in the
 * plain-text form, since the Markdown renderer takes a string), and one line
 * under the text counts them with the title's, which the head shows
 * highlighted. Confirm adopts the candidate's aliases, tags and fact key
 * with its text, and the extractor writes them from model output, so they
 * are drawn under the text, escaped and highlighted, and counted in the same
 * line — the number `openalpaca memory show` counts, plus the replaced
 * note's title, which it does not print. The text is drawn `wrapAnywhere`:
 * a code block wraps, and a table is drawn as its rows, one wrapping line
 * each with every cell kept, so no counted escape sits behind a sideways
 * scroll. A note already in force is rendered as written: a joiner or a
 * right-to-left mark can be its content.
 */

import { MessageBody } from "@/components/chat";
import { EscapedText, Eyebrow } from "@/components/ui";
import type { MemoryNote } from "@/lib/api/memories";
import {
  type Escaped,
  escapeHidden,
  escapeHiddenLine,
  hiddenNotice,
} from "@/lib/hidden-chars";
import { useUiStore } from "@/stores/ui";
import { relativeTime } from "@/views/library/format";

import {
  candidateNote,
  freshnessNote,
  originLabel,
  savedByAlpaca,
  scopeLabel,
  sourceLabel,
} from "./copy";

export interface NoteTabProps {
  note: MemoryNote;
  /**
   * The title of the note a candidate would replace, once read — its copy
   * then names it (design 09 §3.5); `null` for a candidate that names none.
   */
  replaces?: string | null;
}

/** The strings a frontmatter key holds, or none. */
function strings(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === "string")
    : [];
}

/**
 * What Confirm adopts with a candidate's text besides its title — its
 * aliases, tags and fact key — each escaped as a one-line field.
 */
function adoptedFields(
  frontmatter: Record<string, unknown>,
): { label: string; values: Escaped[] }[] {
  const factKey = frontmatter.fact_key;
  return [
    { label: "Aliases", values: strings(frontmatter.aliases) },
    { label: "Tags", values: strings(frontmatter.tags) },
    {
      label: "Fact key",
      values: typeof factKey === "string" && factKey !== "" ? [factKey] : [],
    },
  ]
    .filter((field) => field.values.length > 0)
    .map((field) => ({
      label: field.label,
      values: field.values.map((value) => escapeHiddenLine(value)),
    }));
}

export function NoteTab({ note, replaces = null }: NoteTabProps) {
  const focusRun = useUiStore((s) => s.focusRun);
  const openMemory = useUiStore((s) => s.openMemory);
  const freshness = freshnessNote(note.freshness);
  const deciding = note.effective_status === "candidate";
  const title = escapeHiddenLine(note.title);
  const was = replaces === null ? null : escapeHiddenLine(replaces);
  const body = deciding ? escapeHidden(note.body) : null;
  const adopted = deciding ? adoptedFields(note.frontmatter) : [];
  const adoptedHidden = adopted
    .flatMap((field) => field.values)
    .reduce((sum, value) => sum + value.hidden, 0);
  const notice =
    body === null
      ? null
      : hiddenNotice(
          title.hidden + (was?.hidden ?? 0) + body.hidden + adoptedHidden,
        );

  return (
    <div className="max-w-[660px]">
      {deciding && (
        <p className="mt-0 mb-[14px] rounded-2xl border border-amber-line bg-amber-surface px-[12px] py-[9px] text-base leading-[1.5] wrap-anywhere text-amber-ink">
          {candidateNote(
            note.origin,
            was?.text ?? null,
            savedByAlpaca(note.origin, note.frontmatter["observed_at"]),
          )}
        </p>
      )}
      {note.effective_status === "superseded" && (
        <p className="mt-0 mb-[14px] text-base leading-[1.5] text-tertiary">
          A newer note replaces this one, so it is left out of answers.
        </p>
      )}

      {/* A candidate's blank test reads the escaped text: `trim` strips
          U+3000, U+00A0, U+FEFF and U+2028, which the escape shows. */}
      {(body?.text ?? note.body).trim() === "" ? (
        <p className="m-0 text-md text-muted-fg">This note has no text.</p>
      ) : (
        <MessageBody
          text={body?.text ?? note.body}
          spacing="assistant"
          wrapAnywhere={deciding}
        />
      )}
      {adopted.length > 0 && (
        <p
          aria-label="Adopted with the text"
          className="mt-[10px] mb-0 text-base leading-[1.5] wrap-anywhere text-secondary"
        >
          {adopted.map((field, index) => (
            <span key={field.label}>
              {index > 0 && <span className="text-faint"> · </span>}
              <span className="text-tertiary">{field.label} </span>
              {field.values.map((value, position) => (
                <span key={position}>
                  {position > 0 && ", "}
                  <EscapedText escaped={value} />
                </span>
              ))}
            </span>
          ))}
        </p>
      )}
      {notice !== null && (
        <p className="mt-[10px] mb-0 text-base leading-[1.5] text-red-ink">
          {notice}
        </p>
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
