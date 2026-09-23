/**
 * `StopDaemonDialog` — the confirmation in front of `Stop daemon` (DESIGN_SPEC
 * §5.5's third overlay, §3.32).
 *
 * **The copy must not promise more than a stop does.** Nothing is drained: a
 * stop cuts every in-flight loop mid-round, and the only repair is the next
 * boot's sweep, which marks each run `interrupted` (restart verb: `Rerun`) and
 * files what the user typed at a running workflow as a follow-up. A streaming
 * reply is lost, and a tool waiting for approval is dropped without running.
 * Nothing stronger — "finishing current work", "resumes where it left off" —
 * is true, so none of it is said. A remote Telegram / Discord user gets
 * silence rather than an error (T32: said here, no farewell is built).
 *
 * **The gate is the count line plus a danger button that is not the default
 * focus** (T33: no typed word — a stop destroys a process and a turn, not
 * data). `Cancel` takes focus and Escape. `Stop daemon` is the existing
 * `dangerGhost` (T25: no filled danger variant the design never drew), and it
 * stays disabled until the dialog's fresh read of what the daemon is doing has
 * settled — the warning being confirmed is the one on screen, not a stale one
 * — and while the stop is in flight.
 */

import { Button, Scrim } from "@/components/ui";
import type { DaemonBusyStatus } from "@/lib/api/types";

/** Shown when the daemon served no `busy` block (an older daemon, T23). */
export const GENERIC_BUSY_LINE =
  "This window cannot tell what the daemon is working on right now. If a workflow is running, it will be cut off.";

/** Shown only when a messaging connector is running (T32). */
export const CONNECTOR_SILENCE_LINE =
  "People messaging OpenAlpaca from Telegram, Discord or iMessage will get no reply and no error while it is stopped — it will simply be silent until you start it again.";

function counted(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/**
 * The busy line, from the counts that are non-zero — `null` when all are
 * zero. `connected_clients` includes this window, so the words are "windows
 * connected", which is true as written.
 */
export function busyLine(busy: DaemonBusyStatus): string | null {
  const parts = [
    busy.running_tasks > 0
      ? counted(busy.running_tasks, "workflow running", "workflows running")
      : null,
    busy.pending_confirmations > 0
      ? counted(
          busy.pending_confirmations,
          "tool waiting for approval",
          "tools waiting for approval",
        )
      : null,
    busy.connected_clients > 0
      ? counted(busy.connected_clients, "window connected", "windows connected")
      : null,
  ].filter((part): part is string => part !== null);
  return parts.length === 0 ? null : `Right now: ${parts.join(" · ")}.`;
}

export interface StopDaemonDialogProps {
  /**
   * `GET /v1/status`'s `busy` block. `null`/`undefined` — the daemon did not
   * serve one — draws the generic line instead of a count.
   */
  busy: DaemonBusyStatus | null | undefined;
  /** A messaging connector is running, so its users will meet silence. */
  connectorsRunning: boolean;
  /** The fresh status read is still in flight. */
  reading: boolean;
  /** The stop has been confirmed and is in flight. */
  stopping: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}

export function StopDaemonDialog({
  busy,
  connectorsRunning,
  reading,
  stopping,
  onCancel,
  onConfirm,
}: StopDaemonDialogProps) {
  const line = reading
    ? "Checking what the daemon is doing…"
    : busy === null || busy === undefined
      ? GENERIC_BUSY_LINE
      : busyLine(busy);
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
        aria-labelledby="stop-daemon-title"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.stopPropagation();
            onCancel();
          }
        }}
        className="w-[520px] max-w-[calc(100%-32px)] rounded-4xl border border-line-popover bg-raised px-[20px] py-[18px] shadow-dialog"
      >
        <h2
          id="stop-daemon-title"
          className="m-0 text-md-plus font-semibold text-ink"
        >
          Stop the OpenAlpaca daemon?
        </h2>

        <p className={paragraph}>
          Everything OpenAlpaca is doing stops immediately. Nothing is finished
          first.
        </p>
        <p className={paragraph}>
          Any workflow that is running right now is cut off mid-step. When you
          start the daemon again it comes back marked &ldquo;interrupted&rdquo;
          — open it in Work and choose Rerun to pick the work back up. Anything
          you typed at a running workflow while it worked is kept and delivered
          on that conversation&rsquo;s next turn.
        </p>
        <p className={paragraph}>
          A chat reply that is streaming right now is lost. A tool waiting for
          your approval is dropped without running.
        </p>
        {connectorsRunning && (
          <p className={paragraph}>{CONNECTOR_SILENCE_LINE}</p>
        )}
        <p className={paragraph}>
          Your conversations, artifacts, uploads and memories are untouched —
          this stops a program, it does not delete anything.
        </p>

        {line !== null && (
          <p
            data-testid="stop-daemon-busy"
            className="m-0 mt-[12px] font-mono text-2xs-plus leading-[1.6] text-ink"
          >
            {line}
          </p>
        )}

        <div className="mt-[16px] flex justify-end gap-[6px]">
          {/* Cancel is the default focus, never Stop: Enter or Space on an
              opened dialog must not be what stops the daemon. */}
          <Button autoFocus variant="secondarySm" onClick={onCancel}>
            Cancel
          </Button>
          <Button
            variant="dangerGhost"
            disabled={reading || stopping}
            onClick={onConfirm}
          >
            {stopping ? "Stopping…" : "Stop daemon"}
          </Button>
        </div>
      </div>
    </Scrim>
  );
}
