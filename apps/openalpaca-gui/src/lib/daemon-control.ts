/**
 * Stopping and starting the daemon from this window (DESIGN_SPEC §5.5,
 * API_MAP §1.2).
 *
 * **Stop is two steps and never one.** `POST /v1/command {"command":
 * "shutdown"}` is bearer-authenticated and answers `200 shutting_down` —
 * an acceptance: the daemon still has up to 10 s of shutdown tail to run.
 * `await_daemon_stopped` (the shell) then waits on the process and its lock,
 * and only its answer is reported. This window signals nothing and kills
 * nothing.
 *
 * **The order is the fix.** The stop intent is set and the socket closed
 * *before* the POST, so no reconnect ladder ever starts against a daemon this
 * window is deliberately stopping — and nothing afterwards respawns it. The
 * way back is an explicit `Start daemon` (`startDaemon`).
 *
 * Every dependency is injected so each branch is a test with no daemon.
 */

import { ApiError } from "./http";
import { requestDaemonShutdown } from "./api/status";
import {
  awaitDaemonStopped,
  setStopIntent,
  type DaemonStopReport,
} from "./connection";
import { daemonEvents } from "./events";

export interface DaemonControlDeps {
  /** Close the socket and stop the ladder (`daemonEvents.disconnect`). */
  disconnect: () => void;
  /** Bootstrap and open the socket (`daemonEvents.connect`). */
  connect: () => Promise<void>;
  /** Climb the ladder without bootstrapping (`daemonEvents.resume`). */
  resume: () => void;
  /** `POST /v1/command {"command":"shutdown"}`. */
  shutdown: () => Promise<unknown>;
  /** The shell's `await_daemon_stopped`. */
  awaitStopped: () => Promise<DaemonStopReport>;
}

export const defaultDaemonControlDeps: DaemonControlDeps = {
  disconnect: () => daemonEvents.disconnect(),
  connect: () => daemonEvents.connect(),
  resume: () => daemonEvents.resume(),
  shutdown: requestDaemonShutdown,
  awaitStopped: awaitDaemonStopped,
};

/** How a stop from this window ended. */
export type StopResult =
  /** The process exited and its lock is free (or nothing was running). */
  | { kind: "stopped" }
  /** Asked, accepted, and still alive 15 s later. */
  | { kind: "still_alive"; pid: number | null }
  /** The process exited, but something still holds the lock. */
  | { kind: "lock_still_held" }
  /** Asked and accepted; the shell could not say whether it is gone. */
  | { kind: "unconfirmed"; message: string }
  /** The daemon answered and did not accept. The window is reconnected. */
  | { kind: "refused"; message: string }
  /** The request never reached the daemon. The window is not stopped. */
  | { kind: "unreachable"; message: string };

function messageOf(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

/**
 * Stop the daemon from this window.
 *
 * `onPosted` fires once the shutdown POST has settled either way — the dialog
 * closes on the POST's response, and the outcome arrives as a toast.
 */
export async function stopDaemon(
  deps: DaemonControlDeps = defaultDaemonControlDeps,
  onPosted: () => void = () => {},
): Promise<StopResult> {
  setStopIntent("stopped_here");
  deps.disconnect();

  try {
    await deps.shutdown();
  } catch (cause) {
    onPosted();
    // Not stopped: this window must go back to being an ordinary window.
    setStopIntent(null);
    if (cause instanceof ApiError && cause.isTransport) {
      // Nothing answered. Bootstrapping here would *spawn* a daemon — the
      // opposite of what was asked — so climb the read-only ladder instead,
      // exactly as for a daemon that died.
      deps.resume();
      return { kind: "unreachable", message: messageOf(cause) };
    }
    // The daemon answered and refused: it is alive and serving, so the
    // bootstrap finds it and spawns nothing.
    await deps.connect();
    return { kind: "refused", message: messageOf(cause) };
  }
  onPosted();

  let report: DaemonStopReport;
  try {
    report = await deps.awaitStopped();
  } catch (cause) {
    return { kind: "unconfirmed", message: messageOf(cause) };
  }
  switch (report.outcome) {
    case "stopped":
    case "not_running":
      return { kind: "stopped" };
    case "still_alive":
      return { kind: "still_alive", pid: report.pid };
    case "lock_still_held":
      return { kind: "lock_still_held" };
  }
}

/**
 * `Start daemon`: clear the stop intent, then bootstrap — which spawns the
 * sidecar when nothing is running — and reopen the socket. A restart changes
 * the port, the token and the instance id, so the caller refetches
 * everything afterwards.
 */
export async function startDaemon(
  deps: Pick<DaemonControlDeps, "connect"> = defaultDaemonControlDeps,
): Promise<void> {
  setStopIntent(null);
  await deps.connect();
}

/** The toast each outcome ends in (the spec's §4.3 copy). */
export function stopToast(result: StopResult): string {
  switch (result.kind) {
    case "stopped":
      return "Daemon stopped.";
    case "still_alive":
      return "The daemon did not stop within 15 seconds. Stop it from a terminal with `openalpaca daemon stop`, then choose Start daemon.";
    case "lock_still_held":
      return "The daemon exited but something still holds its lock. Run `openalpaca daemon status` from a terminal before starting it again.";
    case "unconfirmed":
      return `The daemon accepted the stop, but this window could not confirm it is gone: ${result.message}`;
    case "refused":
      return `The daemon refused to stop: ${result.message}`;
    case "unreachable":
      return "Could not reach the daemon to stop it. Stop it from a terminal with `openalpaca daemon stop`.";
  }
}
