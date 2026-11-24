/**
 * The host this window talks to (Q07), chosen once, on first use:
 *
 *   * inside the Tauri shell (its `isTauri` marker): TauriHost — the app;
 *   * else, in a development build only:
 *       - with `VITE_OPENALPACA_HOST=dev-server`, outside the test run:
 *         DevServerHost, loaded by a dynamic import when first asked, so a
 *         production bundle carries none of it;
 *       - else FixtureHost (the tests' host; a plain `bun run dev` session's,
 *         unanswered unless `VITE_OPENALPACA_HOST=fixture`);
 *   * else NoHost: every command refused as a missing bridge is, no Reveal,
 *     no dialog, no window.
 *
 * A test run never chooses DevServerHost, whatever the developer's shell
 * exports: vitest copies `VITE_*` variables into `import.meta.env`.
 */

import { isTauri } from "@tauri-apps/api/core";

import { connectionAnswer, logTailAnswer, stopReportAnswer } from "./answer";
import { fixtureHost } from "./fixture";
import { createTauriHost } from "./tauri";
import type { HostBridge } from "./types";

let chosen: HostBridge | null = null;

/** The host, chosen on the first call and the same object after it. */
export function host(): HostBridge {
  chosen ??= choose();
  return chosen;
}

function choose(): HostBridge {
  if (isTauri()) return createTauriHost();
  if (
    import.meta.env.DEV &&
    import.meta.env.VITE_OPENALPACA_HOST === "dev-server" &&
    import.meta.env.MODE !== "test"
  ) {
    return lazyDevServerHost();
  }
  if (import.meta.env.DEV && fixtureHost !== null) return fixtureHost;
  return noHost();
}

/**
 * DevServerHost behind a dynamic import: its plain members here, its
 * commands loaded with the module on the first call. A module that fails to
 * load is refused through the same check as any other failure.
 */
function lazyDevServerHost(): HostBridge {
  let loading: Promise<HostBridge> | null = null;
  const load = () =>
    (loading ??= import("./dev-server").then((module) =>
      module.createDevServerHost(),
    ));
  const unloaded = (cause: unknown) => () => {
    throw cause;
  };
  return {
    kind: "dev-server",
    reveal: false,
    dialog: null,
    window: null,
    ensureDaemonRunning: () =>
      load().then(
        (host) => host.ensureDaemonRunning(),
        (cause) => connectionAnswer("ensure_daemon_running", unloaded(cause)),
      ),
    getConnectionInfo: () =>
      load().then(
        (host) => host.getConnectionInfo(),
        (cause) => connectionAnswer("get_connection_info", unloaded(cause)),
      ),
    readDaemonLogTail: (lines) =>
      load().then(
        (host) => host.readDaemonLogTail(lines),
        (cause) => logTailAnswer(unloaded(cause)),
      ),
    awaitDaemonStopped: () =>
      load().then(
        (host) => host.awaitDaemonStopped(),
        (cause) => stopReportAnswer(unloaded(cause)),
      ),
  };
}

/** No shell and no development host: every command refused, nothing drawn. */
function noHost(): HostBridge {
  const absent = () => {
    throw new Error("no host bridge");
  };
  return {
    kind: "none",
    reveal: false,
    dialog: null,
    window: null,
    ensureDaemonRunning: () =>
      connectionAnswer("ensure_daemon_running", absent),
    getConnectionInfo: () => connectionAnswer("get_connection_info", absent),
    readDaemonLogTail: () => logTailAnswer(absent),
    awaitDaemonStopped: () => stopReportAnswer(absent),
  };
}
