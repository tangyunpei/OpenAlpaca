/**
 * The one check every host's answer passes (Q07): what judges a shell
 * command's answer and its refusal, moved here unchanged from
 * `lib/connection.ts` so TauriHost, DevServerHost, FixtureHost and NoHost
 * all answer through the same bytes.
 *
 * Each host hands over the raw answer of the command it stands for — the
 * shell's own form, by the command's name — or its raw rejection: the
 * shell's typed refusal `{ kind, message }` (`lib/shell-error.ts`, Q13)
 * becomes a `ConnectionError` carrying that `kind` and the shell's sentence
 * as it is; a bare string is kept as it is with no `kind`; anything else is
 * named by the command.
 *
 * A leaf: it imports `lib/shell-error.ts` and types only, so no host module
 * reaches `lib/connection.ts` at run time.
 */

import type {
  ConnectionInfo,
  DaemonPath,
  DaemonStopReport,
} from "../connection";
import { shellErrorOf, type ShellErrorKind } from "../shell-error";

/** `value` as a `DaemonPath`, or `null` when it is not one. */
export function daemonPathOf(value: unknown): DaemonPath | null {
  if (typeof value !== "object" || value === null) return null;
  const v = value as Record<string, unknown>;
  if (
    v.source !== "login-shell" &&
    v.source !== "login-shell-noninteractive" &&
    v.source !== "fallback-list"
  ) {
    return null;
  }
  if (typeof v.captureMs !== "number" || !Number.isFinite(v.captureMs)) {
    return null;
  }
  if (v.reason !== null && typeof v.reason !== "string") return null;
  if (
    !Array.isArray(v.passedKeys) ||
    !v.passedKeys.every((key) => typeof key === "string")
  ) {
    return null;
  }
  return {
    source: v.source,
    captureMs: v.captureMs,
    reason: v.reason,
    passedKeys: [...(v.passedKeys as string[])],
  };
}

/** Thrown when discovery is missing/expired or the Tauri bridge is absent. */
export class ConnectionError extends Error {
  /** The underlying Tauri rejection or malformed payload. */
  readonly detail: unknown;
  /**
   * What refused, when the shell did (its typed refusal's `kind`); `null`
   * for Tauri's own refusal, an absent bridge or a malformed payload.
   */
  readonly kind: ShellErrorKind | null;
  override readonly name = "ConnectionError";

  constructor(
    message: string,
    detail: unknown = null,
    kind: ShellErrorKind | null = null,
  ) {
    super(message);
    this.detail = detail;
    this.kind = kind;
  }
}

/**
 * A command's rejection as a `ConnectionError`: the shell's typed refusal
 * keeps its `kind` and its sentence; Tauri's own refusal is a bare string,
 * kept as it is with no `kind`; anything else is named by the command.
 */
function refusalOf(command: string, cause: unknown): ConnectionError {
  const shell = shellErrorOf(cause);
  if (shell) return new ConnectionError(shell.message, cause, shell.kind);
  return new ConnectionError(
    typeof cause === "string" ? cause : `Tauri command \`${command}\` failed`,
    cause,
  );
}

function isConnectionInfo(value: unknown): value is ConnectionInfo {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.baseUrl === "string" &&
    typeof v.token === "string" &&
    typeof v.instanceId === "string"
  );
}

/** The two commands that answer a `ConnectionInfo`. */
export type ConnectionCommand = "get_connection_info" | "ensure_daemon_running";

/**
 * `command`'s answer, asked by `ask`, as a `ConnectionInfo`: the shell's
 * fields read as they always were, a refusal or an unexpected payload a
 * `ConnectionError`.
 */
export async function connectionAnswer(
  command: ConnectionCommand,
  ask: () => unknown,
): Promise<ConnectionInfo> {
  let raw: unknown;
  try {
    raw = await ask();
  } catch (cause) {
    throw refusalOf(command, cause);
  }
  if (!isConnectionInfo(raw)) {
    throw new ConnectionError(
      `\`${command}\` returned an unexpected payload`,
      raw,
    );
  }
  const v = raw as unknown as Record<string, unknown>;
  return {
    baseUrl: raw.baseUrl,
    token: raw.token,
    instanceId: raw.instanceId,
    daemonPath: daemonPathOf(raw.daemonPath),
    version: typeof v.version === "string" ? v.version : null,
    protocol:
      typeof v.protocol === "number" && Number.isFinite(v.protocol)
        ? v.protocol
        : null,
    note: typeof v.note === "string" ? v.note : null,
  };
}

/** `read_daemon_log_tail`'s answer, asked by `ask`: a string, or refused. */
export async function logTailAnswer(ask: () => unknown): Promise<string> {
  let raw: unknown;
  try {
    raw = await ask();
  } catch (cause) {
    throw refusalOf("read_daemon_log_tail", cause);
  }
  if (typeof raw !== "string") {
    throw new ConnectionError(
      "`read_daemon_log_tail` returned an unexpected payload",
      raw,
    );
  }
  return raw;
}

const STOP_OUTCOMES: ReadonlySet<string> = new Set([
  "not_running",
  "stopped",
  "lock_still_held",
  "still_alive",
]);

/** `await_daemon_stopped`'s answer, asked by `ask`: a report, or refused. */
export async function stopReportAnswer(
  ask: () => unknown,
): Promise<DaemonStopReport> {
  let raw: unknown;
  try {
    raw = await ask();
  } catch (cause) {
    throw refusalOf("await_daemon_stopped", cause);
  }
  const report = raw as Partial<DaemonStopReport> | null;
  if (
    typeof report !== "object" ||
    report === null ||
    typeof report.outcome !== "string" ||
    !STOP_OUTCOMES.has(report.outcome)
  ) {
    throw new ConnectionError(
      "`await_daemon_stopped` returned an unexpected payload",
      raw,
    );
  }
  return {
    outcome: report.outcome,
    pid: typeof report.pid === "number" ? report.pid : null,
    waitedMs: typeof report.waitedMs === "number" ? report.waitedMs : 0,
  };
}
