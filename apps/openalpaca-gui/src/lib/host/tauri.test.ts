/**
 * TauriHost (Q07) through the real `invoke` path: Tauri's own IPC mocks
 * stand in for the shell, so each command goes out by its name with its
 * arguments and each answer comes back through the shared answer check
 * (`lib/host/answer.ts`), exactly as `lib/connection.ts` read it before.
 */

import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";

import shellErrors from "../shell-errors.json";

import { ConnectionError } from "./answer";
import { createTauriHost } from "./tauri";

/** Every IPC call the host made, by command, with its arguments. */
let calls: Array<[string, unknown]> = [];

/** The shell answering `answer` (or refusing, when it throws) to every call. */
function shell(answer: (command: string, args: unknown) => unknown): void {
  calls = [];
  mockIPC((command, args) => {
    calls.push([command, args]);
    return answer(command, args);
  });
}

/** The rejection `promise` ends in, as a `ConnectionError`. */
async function rejection(promise: Promise<unknown>): Promise<ConnectionError> {
  const cause = await promise.then(
    () => {
      throw new Error("resolved; a refusal was expected");
    },
    (error: unknown) => error,
  );
  expect(cause).toBeInstanceOf(ConnectionError);
  return cause as ConnectionError;
}

const INFO = {
  baseUrl: "http://127.0.0.1:51823",
  daemonPath: null,
  instanceId: "i-1",
  note: null,
  protocol: 1,
  token: "t",
  version: "0.1.0",
};

afterEach(() => {
  clearMocks();
});

describe("TauriHost", () => {
  it("is the shell's: kind tauri, Reveal drawn, a dialog and a window", () => {
    const host = createTauriHost();
    expect(host.kind).toBe("tauri");
    expect(host.reveal).toBe(true);
    expect(host.dialog).not.toBeNull();
    expect(host.window).not.toBeNull();
  });

  it("asks each of the shell's four commands by its name, with its arguments", async () => {
    shell((command) => {
      if (command === "read_daemon_log_tail") return "one\ntwo";
      if (command === "await_daemon_stopped") {
        return { outcome: "stopped", pid: null, waitedMs: 120 };
      }
      return INFO;
    });
    const host = createTauriHost();

    expect(await host.ensureDaemonRunning()).toEqual(INFO);
    expect(await host.getConnectionInfo()).toEqual(INFO);
    expect(await host.readDaemonLogTail(5)).toBe("one\ntwo");
    expect(await host.awaitDaemonStopped()).toEqual({
      outcome: "stopped",
      pid: null,
      waitedMs: 120,
    });
    expect(calls).toEqual([
      ["ensure_daemon_running", {}],
      ["get_connection_info", {}],
      ["read_daemon_log_tail", { lines: 5 }],
      ["await_daemon_stopped", {}],
    ]);
  });

  it("needs no shell marker when it is built directly", async () => {
    expect((globalThis as { isTauri?: unknown }).isTauri).toBeUndefined();
    shell(() => INFO);
    expect((await createTauriHost().getConnectionInfo()).token).toBe("t");
  });

  it("reads the shell's answer as the window always has: unknown fields dropped, absent ones null", async () => {
    shell(() => ({
      baseUrl: "http://127.0.0.1:51823",
      token: "t",
      instanceId: "i-1",
      version: 1,
      protocol: "1",
      note: 7,
      extra: "x",
    }));
    expect(await createTauriHost().ensureDaemonRunning()).toEqual({
      baseUrl: "http://127.0.0.1:51823",
      token: "t",
      instanceId: "i-1",
      daemonPath: null,
      version: null,
      protocol: null,
      note: null,
    });
  });

  it("delivers a refusal the handler throws to invoke as the thrown value: each of the shell's rows keeps its kind and sentence", async () => {
    const host = createTauriHost();
    for (const row of shellErrors.refusals) {
      shell(() => {
        throw row;
      });
      const error = await rejection(host.ensureDaemonRunning());
      expect(error.kind).toBe(row.kind);
      expect(error.message).toBe(row.message);
      expect(error.detail).toBe(row);
    }
  });

  it("keeps Tauri's own refusal, a bare string, with no kind", async () => {
    shell(() => {
      throw "command read_daemon_log_tail missing required key lines";
    });
    const error = await rejection(createTauriHost().readDaemonLogTail(5));
    expect(error.kind).toBeNull();
    expect(error.message).toBe(
      "command read_daemon_log_tail missing required key lines",
    );
  });

  it("refuses an unexpected payload, naming the command", async () => {
    const host = createTauriHost();
    shell(() => ({ baseUrl: "http://127.0.0.1:51823" }));
    expect((await rejection(host.getConnectionInfo())).message).toBe(
      "`get_connection_info` returned an unexpected payload",
    );
    shell(() => 7);
    expect((await rejection(host.readDaemonLogTail(5))).message).toBe(
      "`read_daemon_log_tail` returned an unexpected payload",
    );
    shell(() => ({ outcome: "gone" }));
    expect((await rejection(host.awaitDaemonStopped())).message).toBe(
      "`await_daemon_stopped` returned an unexpected payload",
    );
  });

  it("reaches the window: asks whether it is decorated, closes, minimizes and zooms it", async () => {
    mockWindows("main");
    shell((command) =>
      command === "plugin:window|is_decorated" ? true : null,
    );
    const window = createTauriHost().window;

    expect(await window?.isDecorated()).toBe(true);
    await window?.close();
    await window?.minimize();
    await window?.toggleMaximize();

    expect(calls).toEqual([
      ["plugin:window|is_decorated", { label: "main" }],
      ["plugin:window|close", { label: "main" }],
      ["plugin:window|minimize", { label: "main" }],
      ["plugin:window|toggle_maximize", { label: "main" }],
    ]);
  });

  it("opens a directory picker and answers the folder chosen, or null", async () => {
    const dialog = createTauriHost().dialog;

    shell(() => "/Users/me/plugins/notes");
    expect(await dialog?.pickDirectory()).toBe("/Users/me/plugins/notes");
    expect(calls).toEqual([
      ["plugin:dialog|open", { options: { directory: true, multiple: false } }],
    ]);

    shell(() => null);
    expect(await dialog?.pickDirectory()).toBeNull();

    shell(() => ["/Users/me/plugins/notes"]);
    expect(await dialog?.pickDirectory()).toBeNull();
  });
});
