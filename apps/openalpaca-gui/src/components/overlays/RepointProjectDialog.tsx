/**
 * `RepointProjectDialog` — `Re-point…` on a project in Settings →
 * Connection → Projects (storage design §6 *Re-point*, ST-7, ST-20).
 *
 * A project is addressed by its root in five columns (R14), so a folder
 * moved or renamed outside OpenAlpaca strands its conversations, runs, files
 * and queued follow-ups at the old path; its notes are keyed by the
 * project's id and follow it with no rewrite. The daemon re-points all of it
 * in one transaction, but only the owner knows where the folder went — so
 * this asks, and shows what is recorded at the old root first.
 *
 * Built on `StopDaemonDialog`'s shell: the palette's veil and dialog, Escape
 * cancels. The confirm is `primarySm`, not the danger ghost — a re-point
 * deletes nothing and is undone by re-pointing back — and the folder field
 * takes the focus. `Re-point` stays off until the count on screen has been
 * read, and after a read that failed; while a run under the root is in
 * flight — queued, running or paused (the daemon answers
 * `409 WORKSPACE_BUSY` for each and changes nothing); and
 * for a relative path, which the daemon refuses, or one where the project
 * already is.
 * Every other refusal — `WORKSPACE_EXISTS` above all (ST-20: re-pointing onto
 * another project would merge two) — is the daemon's sentence, shown as
 * written, and the dialog stays open on it.
 *
 * The count is read by the card's root, and its answer names the project the
 * daemon read that root as. When that is not the card's project (R1: a root
 * renamed by case, or moved with a link left behind, now reaches another
 * project — or none), the counts are not this project's, `Re-point` stays
 * off, and the dialog says so instead of drawing them.
 */

import { Button, Scrim } from "@/components/ui";
import type { Project, Workspace, WorkspaceCounts } from "@/lib/api/workspaces";
import {
  isAbsolutePath,
  normalizeProjectPath,
  RELATIVE_PATH_COPY,
} from "@/stores/project";

import { counted } from "./StopDaemonDialog";

/** What is recorded under a root, in the owner's words — non-zero counts only. */
export function rowsLine(rows: WorkspaceCounts): string {
  const parts = [
    rows.sessions > 0
      ? counted(rows.sessions, "conversation", "conversations")
      : null,
    rows.messages > 0 ? counted(rows.messages, "message", "messages") : null,
    rows.tasks > 0 ? counted(rows.tasks, "run", "runs") : null,
    rows.artifacts > 0 ? counted(rows.artifacts, "file", "files") : null,
    rows.followups > 0
      ? counted(rows.followups, "queued follow-up", "queued follow-ups")
      : null,
  ].filter((part): part is string => part !== null);
  return parts.length === 0 ? "nothing yet" : parts.join(" · ");
}

/**
 * What the dialog says instead of the counts when the card's root no longer
 * reaches the card's project (R1), or `null` when it does.
 */
export function foreignRootLine(
  project: Project,
  read: Pick<Workspace, "id" | "root">,
): string | null {
  if (read.id === project.id) return null;
  return read.id === null || read.root === null
    ? `${project.root} no longer reaches this project: a re-point would not move it. Nothing was changed — close this and look at the list again.`
    : `${project.root} now reaches another project, ${read.root}: a re-point would move that one, not this one. Nothing was changed — close this and look at the list again.`;
}

/** The folder a re-point would send, or `null` while the press must stay off. */
export function repointTarget(value: string, project: Project): string | null {
  const target = normalizeProjectPath(value);
  if (target === null || target === project.root) return null;
  return target;
}

export interface RepointProjectDialogProps {
  project: Project;
  /** `GET /v1/workspaces?path=<root>`'s counts; `undefined` until read. */
  rows: WorkspaceCounts | undefined;
  /**
   * The project that read answered for — its `id` and `root` — or
   * `undefined` until read.
   */
  resolvedAs: Pick<Workspace, "id" | "root"> | undefined;
  /**
   * Runs under the root still in flight — queued, running or paused
   * (`queued_tasks` + `active_tasks`): the daemon refuses the re-point for
   * each kind (R10).
   */
  runsInFlight: number;
  /** The fresh read of the counts is still out. */
  reading: boolean;
  /** Why the counts could not be read, or `null`. */
  readError: string | null;
  /** The new folder, as typed or picked. */
  value: string;
  onChange: (value: string) => void;
  /** Open the native folder picker. */
  onBrowse: () => void;
  /** The daemon's refusal of the last press, as written, or `null`. */
  refusal: string | null;
  /** The re-point is in flight. */
  repointing: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}

export function RepointProjectDialog({
  project,
  rows,
  resolvedAs,
  runsInFlight,
  reading,
  readError,
  value,
  onChange,
  onBrowse,
  refusal,
  repointing,
  onCancel,
  onConfirm,
}: RepointProjectDialogProps) {
  const trimmed = value.trim();
  const relative = trimmed !== "" && !isAbsolutePath(trimmed);
  const target = repointTarget(value, project);
  const foreign =
    reading || readError !== null || resolvedAs === undefined
      ? null
      : foreignRootLine(project, resolvedAs);
  const busy = foreign === null && runsInFlight > 0;
  // A failed read leaves no count, and so no busy gate either (S5b M2).
  const canConfirm =
    !reading &&
    readError === null &&
    foreign === null &&
    !busy &&
    !repointing &&
    target !== null;
  const countLine = reading
    ? "Counting what is recorded there…"
    : readError !== null
      ? `Could not count what is recorded there: ${readError}`
      : rows === undefined || foreign !== null
        ? null
        : `Recorded there: ${rowsLine(rows)}.`;
  const paragraph = "m-0 mt-[10px] text-base leading-[1.6] text-secondary";

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
        aria-labelledby="repoint-project-title"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.stopPropagation();
            onCancel();
          }
        }}
        className="w-[520px] max-w-[calc(100%-32px)] rounded-4xl border border-line-popover bg-raised px-[20px] py-[18px] shadow-dialog"
      >
        <h2
          id="repoint-project-title"
          className="m-0 text-md-plus font-semibold text-ink"
        >
          Re-point this project
        </h2>

        <p className={paragraph}>
          {project.missing ? "Its folder is gone from " : "It is recorded at "}
          <span className="font-mono text-2xs-plus text-ink">
            {project.root}
          </span>
          . Name the folder it is in now: its conversations, runs, files and
          notes follow it there. Nothing in either folder is touched.
        </p>

        {countLine !== null && (
          <p
            data-testid="repoint-counts"
            className="m-0 mt-[12px] font-mono text-2xs-plus leading-[1.6] text-ink"
          >
            {countLine}
          </p>
        )}

        {foreign !== null && (
          <p className="m-0 mt-[12px] text-base leading-[1.6] text-red-ink">
            {foreign}
          </p>
        )}

        {busy && (
          <p className={paragraph}>
            {`${counted(runsInFlight, "run there is", "runs there are")} still in flight — a re-point is refused while one is; retry once it finishes.`}
          </p>
        )}

        <div className="mt-[14px] flex flex-wrap items-center gap-[6px]">
          <input
            autoFocus
            aria-label="New project folder"
            placeholder="/Users/you/code/the-project"
            value={value}
            onChange={(event) => onChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && canConfirm) onConfirm();
            }}
            className="min-w-[280px] flex-1 rounded-md border border-line bg-raised px-[8px] py-[4px] font-mono text-2xs-plus text-ink"
          />
          <Button variant="ghostSm" onClick={onBrowse}>
            Choose…
          </Button>
        </div>

        {relative && <p className={paragraph}>{RELATIVE_PATH_COPY}</p>}

        {refusal !== null && (
          <p className="m-0 mt-[10px] text-base leading-[1.6] text-red-ink">
            {refusal}
          </p>
        )}

        <div className="mt-[16px] flex justify-end gap-[6px]">
          <Button variant="secondarySm" onClick={onCancel}>
            Cancel
          </Button>
          <Button
            variant="primarySm"
            disabled={!canConfirm}
            onClick={onConfirm}
          >
            {repointing ? "Re-pointing…" : "Re-point"}
          </Button>
        </div>
      </div>
    </Scrim>
  );
}
