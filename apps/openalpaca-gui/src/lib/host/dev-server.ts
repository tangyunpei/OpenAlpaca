/**
 * DevServerHost (Q07): a plain browser's host with
 * `VITE_OPENALPACA_HOST=dev-server`, in a development build only, reached
 * by a dynamic import from `lib/host/index.ts` so nothing of it is in a
 * production bundle.
 *
 * It asks the Vite dev server's own two endpoints on the page's origin
 * (`scripts/dev-server-host.ts`): the dev server, not the webview, reads
 * the `discovery.json` and `daemon.log` of the store `OPENALPACA_HOME_STORE`
 * names. Each answer and refusal passes through `lib/host/answer.ts`: a
 * refusal carrying one of the shell's kinds keeps it, one without keeps its
 * sentence with no kind.
 *
 * A development aid with limits the app does not have: it judges the
 * token's expiry only — no protocol mismatch, no process check, no health
 * probe — spawns no daemon (the boot's command is the same read as the
 * reconnect's), and cannot wait for a stopped daemon to be gone.
 */

import { connectionAnswer, logTailAnswer, stopReportAnswer } from "./answer";
import type { HostBridge } from "./types";

const CONNECTION_ENDPOINT = "/__openalpaca/connection";
const LOG_TAIL_ENDPOINT = "/__openalpaca/log-tail";

/**
 * `endpoint`'s answer to `body`, POSTed on the page's origin; rejected with
 * a refusal's body when it carries a `kind`, with its sentence when it
 * carries only that, else with a sentence of this host's own.
 */
async function post(endpoint: string, body: unknown): Promise<unknown> {
  let response: Response;
  try {
    response = await fetch(endpoint, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch {
    throw "The dev server did not answer.";
  }
  let payload: unknown = null;
  try {
    payload = await response.json();
  } catch {
    payload = null;
  }
  if (response.ok) return payload;
  if (typeof payload === "object" && payload !== null) {
    const refusal = payload as Record<string, unknown>;
    if (typeof refusal.kind === "string") throw refusal;
    if (typeof refusal.message === "string") throw refusal.message;
  }
  throw `The dev server answered HTTP ${response.status}.`;
}

export function createDevServerHost(): HostBridge {
  return {
    kind: "dev-server",
    reveal: false,
    dialog: null,
    window: null,
    ensureDaemonRunning: () =>
      connectionAnswer("ensure_daemon_running", () =>
        post(CONNECTION_ENDPOINT, {}),
      ),
    getConnectionInfo: () =>
      connectionAnswer("get_connection_info", () =>
        post(CONNECTION_ENDPOINT, {}),
      ),
    readDaemonLogTail: (lines) =>
      logTailAnswer(() => post(LOG_TAIL_ENDPOINT, { lines })),
    awaitDaemonStopped: () =>
      stopReportAnswer(() => {
        throw "The dev server cannot wait for the daemon to stop.";
      }),
  };
}
