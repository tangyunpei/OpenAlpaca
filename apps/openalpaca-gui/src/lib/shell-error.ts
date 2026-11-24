/**
 * The Tauri shell's typed refusals (Q13).
 *
 * A shell command refuses with `{ kind, message }`: `kind` names what
 * refused, `message` is the sentence to show as it is, `spawn_failed`
 * carries `log_tail`, what the daemon this start spawned wrote to its log,
 * and `still_starting` the log's tail too; `mismatch` (Q04) carries
 * `daemon_version`, `daemon_protocol`, `shell_protocol` and `remedy`. This
 * window branches on `kind`, never on the sentence's words.
 *
 * Mirrored by hand from `src-tauri/src/lib.rs`'s `ShellError`, and pinned from
 * both sides: `shell-errors.json` holds every variant's wire form, which the
 * shell's test serializes each variant against and `shell-error.test.ts`
 * reads here. A rejection of Tauri's own — an argument missing, a command not
 * admitted — is a bare string, and is not one of these.
 */

/** Every `kind` the shell refuses with. */
export const SHELL_ERROR_KINDS = [
  /** `get_connection_info`: no `discovery.json`. */
  "not_running",
  /** `get_connection_info`: the discovery file's token has expired. */
  "expired",
  /** `get_connection_info`: `discovery.json` could not be read or parsed. */
  "discovery_unreadable",
  /** `ensure_daemon_running`: no daemon came up. */
  "spawn_failed",
  /** `ensure_daemon_running`: the daemon running speaks another protocol. */
  "mismatch",
  /** `ensure_daemon_running`: the daemon is up but does not answer yet. */
  "starting",
  /** `ensure_daemon_running`: it still does not answer, past the budget. */
  "still_starting",
  /** `read_daemon_log_tail`: the log could not be read. */
  "log_unreadable",
  /** `await_daemon_stopped`: the wait could not be run. */
  "stop_unconfirmed",
] as const;

export type ShellErrorKind = (typeof SHELL_ERROR_KINDS)[number];

/** A `mismatch`'s particulars (Q04): what each side speaks, and the way out. */
export interface ShellMismatch {
  /** The daemon's version; `null` when its discovery file does not say. */
  daemonVersion: string | null;
  /** The daemon's protocol; `null` when its file cannot be read for it. */
  daemonProtocol: number | null;
  /** The protocol this app's shell speaks. */
  shellProtocol: number;
  /** The way out, also the end of the sentence. */
  remedy: string;
}

/** A shell refusal as this window keeps it. */
export interface ShellError {
  kind: ShellErrorKind;
  /** The shell's sentence, verbatim. */
  message: string;
  /**
   * `spawn_failed`'s and `still_starting`'s `log_tail`: what the log ends
   * with; else `null`.
   */
  logTail: string | null;
  /** `mismatch`'s particulars; else `null`. */
  mismatch: ShellMismatch | null;
}

const KINDS: ReadonlySet<string> = new Set(SHELL_ERROR_KINDS);

/**
 * `cause` as one of the shell's refusals, or `null` when it is not one —
 * Tauri's own string, an `Error`, or an object whose `kind` this window does
 * not know or whose fields are not the shell's.
 */
export function shellErrorOf(cause: unknown): ShellError | null {
  if (typeof cause !== "object" || cause === null) return null;
  const v = cause as Record<string, unknown>;
  if (typeof v.kind !== "string" || !KINDS.has(v.kind)) return null;
  if (typeof v.message !== "string") return null;
  let logTail: string | null = null;
  if (v.kind === "spawn_failed" || v.kind === "still_starting") {
    if (typeof v.log_tail === "string") logTail = v.log_tail;
    else if (v.log_tail !== null && v.log_tail !== undefined) return null;
  }
  let mismatch: ShellMismatch | null = null;
  if (v.kind === "mismatch") {
    mismatch = mismatchOf(v);
    if (mismatch === null) return null;
  }
  return {
    kind: v.kind as ShellErrorKind,
    message: v.message,
    logTail,
    mismatch,
  };
}

/** A `mismatch` refusal's particulars, or `null` when they are not the shell's. */
function mismatchOf(v: Record<string, unknown>): ShellMismatch | null {
  const nullable = <T>(value: unknown, is: (x: unknown) => x is T) =>
    value === null || value === undefined
      ? null
      : is(value)
        ? value
        : undefined;
  const isString = (x: unknown) => typeof x === "string";
  const isNumber = (x: unknown): x is number =>
    typeof x === "number" && Number.isFinite(x);
  const daemonVersion = nullable(v.daemon_version, isString);
  const daemonProtocol = nullable(v.daemon_protocol, isNumber);
  if (daemonVersion === undefined || daemonProtocol === undefined) return null;
  if (!isNumber(v.shell_protocol) || !isString(v.remedy)) return null;
  return {
    daemonVersion,
    daemonProtocol,
    shellProtocol: v.shell_protocol,
    remedy: v.remedy,
  };
}
