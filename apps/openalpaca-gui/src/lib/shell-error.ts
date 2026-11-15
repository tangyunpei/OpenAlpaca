/**
 * The Tauri shell's typed refusals (Q13).
 *
 * A shell command refuses with `{ kind, message }`: `kind` names what
 * refused, `message` is the sentence to show as it is, and `spawn_failed`
 * carries `log_tail`, what the daemon this start spawned wrote to its log.
 * This window branches on `kind`, never on the sentence's words.
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
  /** `read_daemon_log_tail`: the log could not be read. */
  "log_unreadable",
  /** `await_daemon_stopped`: the wait could not be run. */
  "stop_unconfirmed",
  /** `reveal_path`: a path the shell does not show. */
  "reveal_refused",
  /** `reveal_path`: the platform could not show it. */
  "reveal_failed",
] as const;

export type ShellErrorKind = (typeof SHELL_ERROR_KINDS)[number];

/** A shell refusal as this window keeps it. */
export interface ShellError {
  kind: ShellErrorKind;
  /** The shell's sentence, verbatim. */
  message: string;
  /** `spawn_failed`'s `log_tail`: what this start's daemon wrote; else `null`. */
  logTail: string | null;
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
  if (v.kind === "spawn_failed") {
    if (typeof v.log_tail === "string") logTail = v.log_tail;
    else if (v.log_tail !== null && v.log_tail !== undefined) return null;
  }
  return { kind: v.kind as ShellErrorKind, message: v.message, logTail };
}
