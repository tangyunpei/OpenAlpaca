/**
 * TauriHost (Q07): the app's own host, the Tauri shell.
 *
 * The shell's four commands, the names read from `src-tauri/src/lib.rs`'s
 * `tauri::generate_handler!` (what each does: `lib/connection.ts`'s
 * header), each answer and refusal judged by `lib/host/answer.ts`; the
 * native window; and the directory picker of the dialog plugin, loaded
 * only when it is asked for.
 */

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { connectionAnswer, logTailAnswer, stopReportAnswer } from "./answer";
import type { HostBridge } from "./types";

export function createTauriHost(): HostBridge {
  return {
    kind: "tauri",
    reveal: true,
    ensureDaemonRunning: () =>
      connectionAnswer("ensure_daemon_running", () =>
        invoke("ensure_daemon_running"),
      ),
    getConnectionInfo: () =>
      connectionAnswer("get_connection_info", () =>
        invoke("get_connection_info"),
      ),
    readDaemonLogTail: (lines) =>
      logTailAnswer(() => invoke("read_daemon_log_tail", { lines })),
    awaitDaemonStopped: () =>
      stopReportAnswer(() => invoke("await_daemon_stopped")),
    dialog: {
      async pickDirectory() {
        const { open } = await import("@tauri-apps/plugin-dialog");
        const picked = await open({ directory: true, multiple: false });
        return typeof picked === "string" ? picked : null;
      },
    },
    window: {
      isDecorated: async () => getCurrentWindow().isDecorated(),
      close: async () => getCurrentWindow().close(),
      minimize: async () => getCurrentWindow().minimize(),
      toggleMaximize: async () => getCurrentWindow().toggleMaximize(),
    },
  };
}
