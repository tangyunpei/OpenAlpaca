/**
 * The host (Q07): everything this window asks of what it runs inside.
 *
 * In the app that is the Tauri shell (TauriHost); in a plain browser it is
 * the Vite dev server, opt-in (DevServerHost), or a fixture (FixtureHost:
 * the tests' host, and a browser session's). `lib/host/index.ts` chooses
 * one, once. Every rejection of every command is a `ConnectionError` made by
 * `lib/host/answer.ts`, carrying the shell's `kind` where the shell (or the
 * dev server, in its words) gives one.
 *
 * The host has no reveal command: the daemon reveals a file itself
 * (`POST /v1/files/{id}/reveal`, Q06). `reveal` only says whether this
 * window draws Reveal at all.
 */

import type { ConnectionInfo, DaemonStopReport } from "../connection";

/** The native directory picker. */
export interface HostDialog {
  /** The folder chosen, or `null` when none was. */
  pickDirectory(): Promise<string | null>;
}

/** The native window this page is drawn in. */
export interface HostWindow {
  isDecorated(): Promise<boolean>;
  close(): Promise<void>;
  minimize(): Promise<void>;
  toggleMaximize(): Promise<void>;
}

export interface HostBridge {
  /** Which host this is. */
  readonly kind: "tauri" | "dev-server" | "fixture" | "none";
  /** Whether this window draws Reveal (only the app's window does). */
  readonly reveal: boolean;
  /** `ensure_daemon_running`: the boot's connection. */
  ensureDaemonRunning(): Promise<ConnectionInfo>;
  /** `get_connection_info`: the reconnect's connection. */
  getConnectionInfo(): Promise<ConnectionInfo>;
  /** `read_daemon_log_tail`: the last `lines` lines of the daemon's log. */
  readDaemonLogTail(lines: number): Promise<string>;
  /** `await_daemon_stopped`: the wait for a stopped daemon to be gone. */
  awaitDaemonStopped(): Promise<DaemonStopReport>;
  /** The directory picker; `null` where there is none. */
  readonly dialog: HostDialog | null;
  /** The native window; `null` where there is none. */
  readonly window: HostWindow | null;
}
