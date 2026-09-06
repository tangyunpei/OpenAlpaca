/**
 * `GET /v1/status` — where the daemon keeps things, how it is doing, and
 * **which project this request resolves to** (plan §4.7 item 4, §4.8, GAP-14).
 *
 * One route answers three questions, so the Connection panel asks once:
 * `started_at`/`uptime_secs`, `schema_version` (the open database's, not a
 * compile-time count of migration files), `log_path` (the CLI-managed
 * `daemon.log`, or `null` where none was written), the two size totals, the
 * boot session-log sweep — and the project below.
 *
 * The project half is the one only this client can ask. The daemon holds no per-lane
 * record of a workspace (R22): a turn carries `x-workspace-path` and the daemon
 * walks up to the nearest `.git`/`.openalpaca` to decide which root owns it.
 * That resolution is not reproducible here — the picker's value is free text,
 * checked only for absoluteness, and nothing in the browser can canonicalize a
 * symlink or find a marker directory. So the route is asked the same question a
 * turn asks, with the same header, and its `project_root` is the window's
 * canonical root (ruling R50).
 *
 * `project_root` is `null` for a caller that sent no header, and for a path
 * that resolves to no project at all or to the home store.
 */

import { apiFetch } from "../http";
import type { DaemonStatus } from "./types";

/**
 * `GET /v1/status`.
 *
 * @param workspacePath the raw directory this window is pointed at, sent as
 *   `x-workspace-path` exactly as a turn sends it. `null` sends no header,
 *   which is the honest signal for "this window has no project".
 */
export async function getDaemonStatus(
  workspacePath: string | null = null,
  signal?: AbortSignal,
): Promise<DaemonStatus> {
  return await apiFetch<DaemonStatus>("/v1/status", {
    headers:
      workspacePath === null
        ? undefined
        : { "x-workspace-path": workspacePath },
    signal,
  });
}
