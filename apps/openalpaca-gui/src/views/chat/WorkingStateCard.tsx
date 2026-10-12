/**
 * The chat aside's **Working state** card (design 11 §5, owner decision
 * CQ-7; DESIGN_SPEC §5.1).
 *
 * The conversation's working state is what the daemon's summariser keeps of
 * where the work stands — written when a turn's context is compacted, when
 * older turns scroll out of the model's window, and when the conversation
 * closes — and it is what the model reads in place of the turns it no longer
 * sees. This card shows the person the same thing: topic, goal, what is done,
 * what they asked and had answered, what is decided and open, the next step,
 * and the runs and files it refers to (a task id opens the run in Work, an
 * artifact path the Library).
 *
 * Every string is the summariser's text, so each one goes through
 * `EscapedText`: a hidden character is drawn as `<U+XXXX>`, never acted on.
 * Collapsible, and open by default; it sits above the Work pane in the
 * aside's work mode and goes with the aside.
 */

import { useState } from "react";

import { EscapedText, Eyebrow } from "@/components/ui";
import type { WorkingState } from "@/lib/api/types";
import { escapeHiddenLine } from "@/lib/hidden-chars";
import { relativeTime } from "@/views/library/format";

/** Drawn while the conversation has no state yet. */
export const WORKING_STATE_EMPTY =
  "Nothing summarised yet — the whole conversation is still in view";

/**
 * What a ref opens (the writer's prompt spells them `task 1a2b3c4d` and
 * `artifact reports/q3.md`): a run — the one a full id names, or the Work
 * view when the id is short — or the Library; anything else is plain text.
 */
export type RefTarget =
  { kind: "run"; runId: string | null } | { kind: "library" } | null;

const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i;

export function refTarget(ref: string): RefTarget {
  const text = ref.trim();
  if (/^task\b/i.test(text))
    return { kind: "run", runId: UUID.exec(text)?.[0] ?? null };
  if (/^artifact\b/i.test(text)) return { kind: "library" };
  return null;
}

export interface WorkingStateCardProps {
  /** The conversation's state; `null` while it has none. */
  state: WorkingState | null;
  /** When the state was written (`working_state_updated_at`). */
  updatedAt: string | null;
  /** A `task …` ref: the run a full id names, or `null` for the Work view. */
  onOpenRun: (runId: string | null) => void;
  /** An `artifact …` ref. */
  onOpenLibrary: () => void;
}

function Line({ text }: { text: string }) {
  return <EscapedText escaped={escapeHiddenLine(text)} />;
}

function Field({ label, text }: { label: string; text: string }) {
  if (text === "") return null;
  return (
    <p className="m-0 mb-[6px] text-base leading-[1.45] text-secondary">
      <span className="text-muted-fg">{label}: </span>
      <Line text={text} />
    </p>
  );
}

function List({ label, items }: { label: string; items: readonly string[] }) {
  if (items.length === 0) return null;
  return (
    <div className="mb-[6px]">
      <p className="m-0 text-base text-muted-fg">{label}</p>
      <ul className="m-0 list-disc pl-[18px] text-base leading-[1.45] text-secondary">
        {items.map((item, index) => (
          <li key={index}>
            <Line text={item} />
          </li>
        ))}
      </ul>
    </div>
  );
}

export function WorkingStateCard({
  state,
  updatedAt,
  onOpenRun,
  onOpenLibrary,
}: WorkingStateCardProps) {
  const [open, setOpen] = useState(true);
  const updated =
    state === null || updatedAt === null ? "" : relativeTime(updatedAt);

  return (
    <section
      aria-label="Working state"
      className="flex max-h-[45%] shrink-0 flex-col border-b border-line-strong"
    >
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((was) => !was)}
        className="flex w-full cursor-pointer items-center gap-[8px] border-none bg-transparent px-[18px] py-[10px] text-left focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-blue"
      >
        <Eyebrow size={9.5}>Working state</Eyebrow>
        {updated !== "" && (
          <span className="font-mono text-2xs-plus text-faint">
            updated {updated}
          </span>
        )}
        <span aria-hidden className="ml-auto text-muted-fg">
          {open ? "▾" : "▸"}
        </span>
      </button>

      {open && (
        <div className="sc min-h-0 overflow-y-auto px-[18px] pb-[12px]">
          {state === null ? (
            <p className="m-0 text-base leading-[1.45] text-muted-fg">
              {WORKING_STATE_EMPTY}
            </p>
          ) : (
            <>
              <p className="m-0 mb-[6px] text-md font-medium text-ink">
                <Line text={state.topic} />
              </p>
              <Field label="Goal" text={state.goal} />
              <List label="Done" items={state.done} />
              <List label="Asked" items={state.asked} />
              <List label="Decided" items={state.decisions} />
              <List label="Open" items={state.open} />
              <Field label="Next" text={state.next} />
              {state.refs.length > 0 && (
                <ul className="m-0 flex list-none flex-wrap gap-[6px] p-0">
                  {state.refs.map((ref, index) => {
                    const target = refTarget(ref);
                    const label = <Line text={ref} />;
                    return (
                      <li
                        key={index}
                        className="font-mono text-2xs-plus text-secondary"
                      >
                        {target === null ? (
                          label
                        ) : (
                          <button
                            type="button"
                            onClick={() =>
                              target.kind === "run"
                                ? onOpenRun(target.runId)
                                : onOpenLibrary()
                            }
                            className="cursor-pointer border-none bg-transparent p-0 font-mono text-2xs-plus text-ink underline decoration-line-strong underline-offset-2 hover:decoration-ink"
                          >
                            {label}
                          </button>
                        )}
                      </li>
                    );
                  })}
                </ul>
              )}
            </>
          )}
        </div>
      )}
    </section>
  );
}
