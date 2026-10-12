/**
 * Settings → Persona — the `USER.md` editor (memory redesign, design §6
 * *USER.md*, §7a `GET|PUT /v1/persona/user`; owner decisions MD-3 and MD-9).
 *
 * `USER.md` is the owner's standing instructions about themselves, and it
 * rides in every prompt. The background learner stopped writing into it at the
 * cutover — what Alpaca learns is a memory note now — so this editor, and
 * `openalpaca persona edit user`, are how it changes. The lines the old
 * learner already wrote are still in it and are the owner's to keep or delete
 * (G38); nothing here sorts them.
 *
 * **A draft is never lost to a conflict.** The editor sends the revision it
 * opened as `If-Match`; the onboarding `update_persona` or the CLI may have
 * written the file since, and then the save is a `409` the section states over
 * the kept draft. `Reload` shows the current file above the draft and the next
 * save goes against the version the owner has now seen. The sentence is this
 * window's, not the daemon's — the route's message names revisions.
 */

import { useState } from "react";

import { Button } from "@/components/ui";
import { usePersonaUser, useSavePersonaUser } from "@/hooks/usePersona";
import { isPersonaConflict, type PersonaUser } from "@/lib/api/persona";
import { useUiStore } from "@/stores/ui";

import { Card, GapNote } from "./primitives";

/** Design §7's conflict sentence, for the file this section edits. */
export const PERSONA_CONFLICT_COPY =
  "USER.md changed since you opened it. Reload to see the current version — your text is kept.";

interface PersonaDraft {
  text: string;
  /** The version the draft is an edit of — its revision is `If-Match`. */
  base: PersonaUser;
  /** The current file, once the owner reloaded over a conflict. */
  current: PersonaUser | null;
  /** The save came back `409`. */
  refused: boolean;
  error: string | null;
}

export function PersonaSection() {
  const persona = usePersonaUser();
  const save = useSavePersonaUser();
  const showToast = useUiStore((s) => s.showToast);
  const setView = useUiStore((s) => s.setView);
  const [draft, setDraft] = useState<PersonaDraft | null>(null);

  if (persona.error !== null && persona.data === undefined) {
    return (
      <Card>
        <p className="m-0 text-md text-muted-fg">USER.md could not be read.</p>
        <GapNote>{persona.error.message}</GapNote>
      </Card>
    );
  }
  const server = persona.data;
  if (server === undefined) {
    return (
      <Card>
        <p className="m-0 text-md text-muted-fg">Reading USER.md…</p>
      </Card>
    );
  }

  const text = draft?.text ?? server.content;
  const dirty = draft !== null && draft.text !== draft.base.content;
  const conflict =
    draft !== null && (draft.refused || server.etag !== draft.base.etag);

  const edit = (value: string) =>
    setDraft((d) => ({
      text: value,
      base: d?.base ?? server,
      current: d?.current ?? null,
      refused: d?.refused ?? false,
      error: null,
    }));

  const onSave = () => {
    if (draft === null) return;
    save.mutate(
      { content: draft.text, etag: draft.base.etag },
      {
        onSuccess: () => {
          setDraft(null);
          showToast("USER.md saved");
        },
        onError: (error) =>
          setDraft((d) =>
            d === null
              ? d
              : isPersonaConflict(error)
                ? { ...d, refused: true, error: null }
                : { ...d, error: error.message },
          ),
      },
    );
  };

  const onReload = () => {
    void persona.refetch().then((result) => {
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
    <div className="flex flex-col gap-[12px]">
      <p className="m-0 text-base leading-[1.6] text-secondary">
        Write it the way you would brief someone who works with you: how to
        address you, how you like answers, what to always keep in mind. Facts
        Alpaca picks up along the way are notes in{" "}
        <button
          type="button"
          onClick={() => setView("memory")}
          className="cursor-pointer border-none bg-transparent p-0 text-base text-blue underline hover:text-blue-hover"
        >
          Memory
        </button>
        , where you can confirm or undo each one.
      </p>

      {conflict && (
        <div
          role="alert"
          className="flex flex-wrap items-center gap-[10px] rounded-2xl border border-amber-line bg-amber-surface px-[12px] py-[9px]"
        >
          <span className="min-w-0 flex-1 text-base leading-[1.5] text-amber-ink">
            {PERSONA_CONFLICT_COPY}
          </span>
          <Button variant="outlineRaised" onClick={onReload}>
            Reload
          </Button>
        </div>
      )}

      {draft?.current != null && (
        <section
          aria-label="Current version"
          className="rounded-md border border-line-subtle bg-sunken px-[12px] py-[10px]"
        >
          <p className="mt-0 mb-[6px] font-mono text-2xs-plus text-faint uppercase">
            Current version
          </p>
          <pre className="m-0 font-mono text-sm leading-[1.6] whitespace-pre-wrap text-secondary">
            {draft.current.content}
          </pre>
        </section>
      )}

      <textarea
        aria-label="USER.md"
        value={text}
        rows={20}
        spellCheck={false}
        onChange={(event) => edit(event.target.value)}
        className="w-full resize-y rounded-2xl border border-line bg-raised px-[14px] py-[12px] font-mono text-sm leading-[1.6] text-ink"
      />

      {draft?.error != null && (
        <p className="m-0 text-base leading-[1.5] text-red-ink">
          Could not save — {draft.error}
        </p>
      )}

      <div className="flex items-center gap-[8px]">
        <Button
          variant="primarySm"
          disabled={!dirty || conflict || save.isPending}
          onClick={onSave}
        >
          Save
        </Button>
        <Button
          variant="ghostSm"
          disabled={draft === null}
          onClick={() => setDraft(null)}
        >
          Revert
        </Button>
      </div>
    </div>
  );
}
