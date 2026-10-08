/**
 * `DeleteProjectDataDialog` — `Delete data…` on a project in Settings →
 * Connection → Projects (storage design §6 *Delete its data*, ST-19).
 *
 * ST-19: everything OpenAlpaca holds for the project goes — conversations,
 * runs, by-products, uploads, the notes of every owner, and its folder under
 * `projects/` — and the project's own folder is never touched; a file
 * another conversation still uses is kept (a by-product as its newest
 * version only). Nothing is deleted before the plan is on screen: the flow
 * asks the daemon for a dry run on open and this draws it as served — its
 * counts, what it keeps, its entries — and `Delete data` stays off until
 * that plan has arrived.
 *
 * The gate is the plan plus the existing danger ghost, never the default
 * focus (`Cancel` is, and Escape cancels), like `StopDaemonDialog`; no word
 * is typed. A refusal — `WORKSPACE_BUSY` while a run is in flight — is the
 * daemon's sentence, and the dialog stays open on it.
 *
 * The plan names the project the daemon read the card's root as. When that
 * is not the card's project (R1: a root renamed by case, or moved with a
 * link left behind, now reaches another project), the plan is another
 * project's: it is not drawn under this card's root, `Delete data` stays
 * off, and the dialog says whose it is.
 */

import { Button, Scrim, Tag } from "@/components/ui";
import type {
  DeleteCounts,
  Project,
  ProjectDataDeletion,
} from "@/lib/api/workspaces";

import { counted } from "./StopDaemonDialog";

const COUNT_WORDS: ReadonlyArray<
  readonly [keyof DeleteCounts, string, string]
> = [
  ["sessions", "conversation", "conversations"],
  ["messages", "message", "messages"],
  ["tasks", "run", "runs"],
  ["spans", "subagent record", "subagent records"],
  ["run_events", "run event", "run events"],
  ["tool_calls", "tool call", "tool calls"],
  ["followups", "queued follow-up", "queued follow-ups"],
  ["uploads", "upload", "uploads"],
  ["artifacts", "by-product", "by-products"],
  ["memory_notes", "memory note", "memory notes"],
];

/** The plan's counts in the owner's words — non-zero ones only. */
export function deleteCountsLine(counts: DeleteCounts): string {
  const parts = COUNT_WORDS.filter(([key]) => counts[key] > 0).map(
    ([key, one, many]) => counted(counts[key], one, many),
  );
  return parts.length === 0
    ? "Nothing is recorded for this project but its folder under projects/."
    : `Deletes ${parts.join(" · ")}.`;
}

/**
 * What the dialog says instead of the plan when the plan is another
 * project's (R1), or `null` when it is the card's own.
 */
export function foreignPlanLine(
  project: Project,
  plan: ProjectDataDeletion,
): string | null {
  if (plan.project.id === project.id) return null;
  return `${project.root} now reaches another project, ${plan.project.root} (projects/${plan.project.name}): this would delete that one's data, not this one's. Nothing was deleted — close this and look at the list again.`;
}

/** What the delete keeps because another conversation still uses it, or `null`. */
export function keptLine(kept: ProjectDataDeletion["kept"]): string | null {
  const parts = [
    kept.uploads > 0 ? counted(kept.uploads, "upload", "uploads") : null,
    kept.artifacts > 0
      ? counted(kept.artifacts, "by-product", "by-products")
      : null,
  ].filter((part): part is string => part !== null);
  return parts.length === 0
    ? null
    : `Keeps ${parts.join(" and ")} another conversation still uses, moved out of the project — a by-product as its newest version only.`;
}

export interface DeleteProjectDataDialogProps {
  project: Project;
  /** The dry run's answer; `undefined` while it is out or after it failed. */
  plan: ProjectDataDeletion | undefined;
  /** Why the dry run failed, as the daemon wrote it, or `null`. */
  planError: string | null;
  /** The delete's refusal, as the daemon wrote it, or `null`. */
  refusal: string | null;
  /** The delete is in flight. */
  deleting: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}

export function DeleteProjectDataDialog({
  project,
  plan,
  planError,
  refusal,
  deleting,
  onCancel,
  onConfirm,
}: DeleteProjectDataDialogProps) {
  const paragraph = "m-0 mt-[10px] text-base leading-[1.6] text-secondary";
  const mono = "m-0 mt-[12px] font-mono text-2xs-plus leading-[1.6] text-ink";
  const foreign = plan === undefined ? null : foreignPlanLine(project, plan);
  const kept = plan === undefined ? null : keptLine(plan.kept);

  return (
    <Scrim
      variant="veil"
      zIndex={50}
      closeOnSelfOnly
      className="fixed"
      onClose={onCancel}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="delete-project-title"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.stopPropagation();
            onCancel();
          }
        }}
        className="w-[560px] max-w-[calc(100%-32px)] rounded-4xl border border-line-popover bg-raised px-[20px] py-[18px] shadow-dialog"
      >
        <h2
          id="delete-project-title"
          className="m-0 text-md-plus font-semibold text-ink"
        >
          Delete this project&rsquo;s data?
        </h2>

        <p className={paragraph}>
          Everything OpenAlpaca holds for{" "}
          <span className="font-mono text-2xs-plus text-ink">
            {project.root}
          </span>{" "}
          is deleted: its conversations, runs, by-products, uploads, the memory
          notes of every owner, and its folder under projects/ in the home
          store.
        </p>
        <p className={paragraph}>
          The project&rsquo;s own folder is never touched. This cannot be
          undone.
        </p>

        {plan === undefined && planError === null && (
          <p data-testid="delete-plan" className={mono}>
            Reading what would be deleted…
          </p>
        )}
        {planError !== null && (
          <p className="m-0 mt-[12px] text-base leading-[1.6] text-red-ink">
            {`Could not read what would be deleted: ${planError}`}
          </p>
        )}
        {foreign !== null && (
          <p className="m-0 mt-[12px] text-base leading-[1.6] text-red-ink">
            {foreign}
          </p>
        )}
        {plan !== undefined && foreign === null && (
          <>
            <p data-testid="delete-plan" className={mono}>
              {deleteCountsLine(plan.counts)}
            </p>
            {kept !== null && <p className={paragraph}>{kept}</p>}
            {plan.entries.length > 0 && (
              <ul
                aria-label="What the delete does"
                className="m-0 mt-[10px] flex list-none flex-col gap-[4px] p-0"
              >
                {plan.entries.map((entry) => (
                  <li
                    key={`${entry.action}:${entry.entry}`}
                    className="flex flex-wrap items-baseline gap-[8px] text-base leading-[1.5] text-secondary"
                  >
                    <Tag
                      value={entry.action}
                      tone={entry.action === "delete" ? "warn" : "neutral"}
                    />
                    <span className="font-mono text-2xs-plus text-ink">
                      {entry.entry}
                    </span>
                    <span>{entry.holds}</span>
                    <span className="font-mono text-2xs-plus text-faint">
                      {entry.retention}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </>
        )}

        {refusal !== null && (
          <p className="m-0 mt-[10px] text-base leading-[1.6] text-red-ink">
            {refusal}
          </p>
        )}

        <div className="mt-[16px] flex justify-end gap-[6px]">
          {/* Cancel is the default focus, never Delete: Enter or Space on an
              opened dialog must not be what deletes a project's data. */}
          <Button autoFocus variant="secondarySm" onClick={onCancel}>
            Cancel
          </Button>
          <Button
            variant="dangerGhost"
            disabled={plan === undefined || foreign !== null || deleting}
            onClick={onConfirm}
          >
            {deleting ? "Deleting…" : "Delete data"}
          </Button>
        </div>
      </div>
    </Scrim>
  );
}
