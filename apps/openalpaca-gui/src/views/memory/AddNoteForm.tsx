/**
 * Add a note by hand — the design's empty state ends "…or add a note".
 *
 * A save here is `POST /v1/memories` from a user-directed surface, so the note
 * is recorded with origin `user`: the highest authority there is (design §4).
 * It goes to the partition the list is showing; a note that is already there
 * word for word is not written twice (exact dedup, §7b P8) — the daemon hands
 * back the existing one, and that is the one opened.
 */

import { useState } from "react";

import { Button } from "@/components/ui";
import { useCreateMemory } from "@/hooks/useMemories";
import { NOTE_KINDS, type NoteKind, type NoteScope } from "@/lib/api/memories";
import { useUiStore } from "@/stores/ui";

const FIELD =
  "w-full rounded-md border border-line bg-raised px-[8px] py-[5px] text-base text-ink";

export interface AddNoteFormProps {
  scope: NoteScope;
  workspacePath: string | null;
  onCancel: () => void;
  onCreated: (id: string) => void;
}

export function AddNoteForm({
  scope,
  workspacePath,
  onCancel,
  onCreated,
}: AddNoteFormProps) {
  const showToast = useUiStore((s) => s.showToast);
  const create = useCreateMemory(workspacePath);
  const [title, setTitle] = useState("");
  const [kind, setKind] = useState<NoteKind>("fact");
  const [body, setBody] = useState("");
  const [error, setError] = useState<string | null>(null);

  const ready = title.trim() !== "" && body.trim() !== "" && !create.isPending;

  const save = () => {
    setError(null);
    create.mutate(
      { title: title.trim(), body, kind, scope },
      {
        onSuccess: (result) => {
          showToast(
            result.dedup === "existing"
              ? "Already remembered — opened the note that says so"
              : `Saved “${title.trim()}”`,
          );
          onCreated(result.id);
        },
        onError: (failure) => setError(failure.message),
      },
    );
  };

  return (
    <section
      aria-label="Add a note"
      className="sc min-w-0 flex-1 overflow-y-auto px-[24px] pt-[16px] pb-[28px]"
    >
      <h2 className="m-0 mb-[4px] text-2xl font-semibold tracking-tighter text-ink">
        Add a note
      </h2>
      <p className="mt-0 mb-[16px] text-md leading-[1.6] text-tertiary">
        Saved to {scope === "project" ? "this project" : "Global"}, as something
        you said yourself.
      </p>

      <div className="flex max-w-[660px] flex-col gap-[10px]">
        <input
          aria-label="Title"
          placeholder="Title"
          value={title}
          onChange={(event) => setTitle(event.target.value)}
          className={FIELD}
        />
        <select
          aria-label="Kind"
          value={kind}
          onChange={(event) => setKind(event.target.value as NoteKind)}
          className={FIELD}
        >
          {NOTE_KINDS.map((value) => (
            <option key={value} value={value}>
              {value}
            </option>
          ))}
        </select>
        <textarea
          aria-label="Body"
          placeholder="What should Alpaca remember? Markdown is fine."
          value={body}
          rows={10}
          onChange={(event) => setBody(event.target.value)}
          className={`${FIELD} resize-y font-mono text-sm leading-[1.6]`}
        />
        {error !== null && (
          <p className="m-0 text-base leading-[1.5] text-red-ink">
            Could not save — {error}
          </p>
        )}
        <div className="flex items-center gap-[8px]">
          <Button variant="primarySm" disabled={!ready} onClick={save}>
            Save note
          </Button>
          <Button variant="ghostSm" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </div>
    </section>
  );
}
