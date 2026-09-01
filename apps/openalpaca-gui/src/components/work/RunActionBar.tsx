/**
 * Run controls (DESIGN_SPEC §3.19 action bar, §3.26 action group + terminal
 * banner).
 *
 * Two sizes of the same catalogue (`run-actions.ts`): small buttons on a
 * card's tinted footer, medium ones under the detail heading. Actions the
 * daemon cannot perform render **disabled with a tooltip**, and the detail
 * additionally lists them in visible text under the group — a tooltip is not
 * evidence, and the gap hand-off is built from what the UI states.
 */

import { Button } from "@/components/ui";
import { cn } from "@/lib/cn";

import {
  unavailableActionNotes,
  type RunActionDescriptor,
  type RunActionId,
} from "./run-actions";

export type ActionBarSize = "card" | "detail";

export interface RunActionBarProps {
  actions: readonly RunActionDescriptor[];
  onAction: (id: RunActionId) => void;
  size: ActionBarSize;
  /** The action currently in flight — its button shows as busy. */
  busy?: RunActionId | null;
  /** Card only: the tighter padding of compact density (§8.3). */
  dense?: boolean;
  className?: string;
}

/** `Cancel` on a card, `Cancel run` in the detail (§3.26). */
function labelFor(action: RunActionDescriptor, size: ActionBarSize): string {
  if (size === "detail" && action.id === "cancel") return "Cancel run";
  return action.label;
}

export function RunActionBar({
  actions,
  onAction,
  size,
  busy = null,
  dense = false,
  className,
}: RunActionBarProps) {
  const detail = size === "detail";

  return (
    <div
      className={cn(
        "flex flex-wrap gap-[6px]",
        detail
          ? "mt-[16px] mb-[22px]"
          : cn(
              "border-t border-line-hair bg-sunken",
              dense ? "px-[13px] py-[8px]" : "px-[15px] py-[10px]",
            ),
        className,
      )}
    >
      {actions.map((action) => (
        <Button
          key={action.id}
          variant={
            action.tone === "danger"
              ? "dangerGhost"
              : detail
                ? "secondaryMd"
                : "secondarySm"
          }
          className={
            detail && action.tone === "danger"
              ? "px-[11px] py-[6px] text-base"
              : undefined
          }
          disabled={!action.enabled || busy === action.id}
          title={action.title}
          onClick={() => onAction(action.id)}
        >
          {labelFor(action, size)}
        </Button>
      ))}
    </div>
  );
}

export interface UnavailableActionsNoteProps {
  actions: readonly RunActionDescriptor[];
  className?: string;
}

/** The visible footnote under the detail action group. */
export function UnavailableActionsNote({
  actions,
  className,
}: UnavailableActionsNoteProps) {
  const notes = unavailableActionNotes(actions);
  if (notes.length === 0) return null;

  return (
    <ul
      className={cn(
        "m-0 mb-[22px] flex list-none flex-col gap-[3px] p-0 font-mono text-2xs-plus leading-[1.5] text-faint",
        className,
      )}
    >
      {notes.map((note) => (
        <li key={note}>{note}</li>
      ))}
    </ul>
  );
}

// ── Terminal treatments ─────────────────────────────────────────────────────

export interface TerminalRunRowProps {
  /** The run's own note, or `null`. */
  note: string | null;
  status: "done" | "cancelled" | "failed";
  onAction: (id: RunActionId) => void;
  rerun: RunActionDescriptor;
  dense?: boolean;
}

/** A finished run's card footer: a mono note and a single `Re-run` (§3.19). */
export function TerminalRunRow({
  note,
  status,
  onAction,
  rerun,
  dense = false,
}: TerminalRunRowProps) {
  return (
    <div
      className={cn(
        "flex w-full items-center gap-[9px] border-t border-line-hair bg-sunken",
        dense ? "px-[13px] py-[8px]" : "px-[15px] py-[10px]",
      )}
    >
      <span className="flex-1 font-mono text-xs leading-[1.5] text-muted-fg">
        {note ?? DEFAULT_TERMINAL_NOTE[status]}
      </span>
      <Button
        variant="secondarySm"
        disabled={!rerun.enabled}
        title={rerun.title}
        onClick={() => onAction(rerun.id)}
      >
        {rerun.label}
      </Button>
    </div>
  );
}

/**
 * Only used when the daemon supplied no summary of its own — it states the
 * status and nothing more, so it cannot be mistaken for a report.
 */
export const DEFAULT_TERMINAL_NOTE: Record<
  "done" | "cancelled" | "failed",
  string
> = {
  done: "finished",
  cancelled: "cancelled by you",
  failed: "failed",
};

export interface TerminalBannerProps {
  status: "done" | "cancelled" | "failed";
  note: string | null;
  actions: readonly RunActionDescriptor[];
  onAction: (id: RunActionId) => void;
}

/** §3.26's banner, which replaces the action group on a finished run. */
export function TerminalBanner({
  status,
  note,
  actions,
  onAction,
}: TerminalBannerProps) {
  const good = status === "done";
  const text =
    status === "done"
      ? `Finished${note === null ? "" : ` · ${note}`}`
      : status === "cancelled"
        ? "Cancelled by you · no further steps will run"
        : `Failed${note === null ? "" : ` · ${note}`}`;

  return (
    <div
      className={cn(
        "mt-[16px] mb-[22px] flex items-center gap-[11px] rounded-xl px-[14px] py-[11px]",
        good
          ? "border border-green-line bg-green-tint"
          : "border border-red-line bg-red-tint",
      )}
    >
      <p
        className={cn(
          "m-0 flex-1 text-base-plus",
          good ? "text-green-ink" : "text-red-ink",
        )}
      >
        {text}
      </p>
      {actions.map((action) => (
        <Button
          key={action.id}
          variant="outlineRaised"
          disabled={!action.enabled}
          title={action.title}
          onClick={() => onAction(action.id)}
        >
          {action.label}
        </Button>
      ))}
    </div>
  );
}
