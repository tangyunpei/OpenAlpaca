/**
 * How `lib/connection.ts` reads the shell's refusals (Q13): a typed refusal
 * `{ kind, message }` becomes a `ConnectionError` carrying that `kind` and
 * the shell's sentence as it is; Tauri's own refusal (a bare string) keeps
 * its sentence with no `kind`; anything else is named by the command.
 */

import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  ConnectionError,
  awaitDaemonStopped,
  bootstrapConnection,
  readDaemonLogTail,
  refreshConnection,
  resetConnection,
} from "./connection";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

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

beforeEach(() => {
  resetConnection();
});

afterEach(() => {
  vi.mocked(invoke).mockReset();
});

describe("a typed refusal of the shell's", () => {
  it("is a ConnectionError with its kind and its sentence, at boot", async () => {
    const message =
      "The daemon did not start within 5 seconds. Its log (/s/state/logs/daemon.log) ends with:\n\nError: this database is newer than this build";
    vi.mocked(invoke).mockRejectedValue({
      kind: "spawn_failed",
      message,
      log_tail: "Error: this database is newer than this build",
    });

    const error = await rejection(bootstrapConnection());

    expect(invoke).toHaveBeenCalledWith("ensure_daemon_running");
    expect(error.kind).toBe("spawn_failed");
    expect(error.message).toBe(message);
  });

  it("is a ConnectionError with its kind and its sentence, on reconnect", async () => {
    vi.mocked(invoke).mockRejectedValue({
      kind: "not_running",
      message: "Discovery not found: daemon may not be running",
    });

    const error = await rejection(refreshConnection());

    expect(invoke).toHaveBeenCalledWith("get_connection_info");
    expect(error.kind).toBe("not_running");
    expect(error.message).toBe(
      "Discovery not found: daemon may not be running",
    );
  });

  it("names an expired token", async () => {
    vi.mocked(invoke).mockRejectedValue({
      kind: "expired",
      message: "Discovery token has expired",
    });

    const error = await rejection(refreshConnection());

    expect(error.kind).toBe("expired");
    expect(error.message).toBe("Discovery token has expired");
  });

  it("names a log the shell could not read", async () => {
    vi.mocked(invoke).mockRejectedValue({
      kind: "log_unreadable",
      message:
        "Could not read /s/state/logs/daemon.log: Is a directory (os error 21)",
    });

    const error = await rejection(readDaemonLogTail(5));

    expect(invoke).toHaveBeenCalledWith("read_daemon_log_tail", { lines: 5 });
    expect(error.kind).toBe("log_unreadable");
    expect(error.message).toBe(
      "Could not read /s/state/logs/daemon.log: Is a directory (os error 21)",
    );
  });

  it("names a stop the shell could not wait for", async () => {
    vi.mocked(invoke).mockRejectedValue({
      kind: "stop_unconfirmed",
      message:
        'Could not wait for the daemon to stop: task 7 panicked with message "x"',
    });

    const error = await rejection(awaitDaemonStopped());

    expect(error.kind).toBe("stop_unconfirmed");
    expect(error.message).toBe(
      'Could not wait for the daemon to stop: task 7 panicked with message "x"',
    );
  });
});

describe("a refusal that is not the shell's", () => {
  it("keeps Tauri's own sentence, a bare string, with no kind", async () => {
    vi.mocked(invoke).mockRejectedValue(
      "command read_daemon_log_tail missing required key lines",
    );

    const error = await rejection(readDaemonLogTail(5));

    expect(error.kind).toBeNull();
    expect(error.message).toBe(
      "command read_daemon_log_tail missing required key lines",
    );
  });

  it("is named by its command when the shell's answer is unknown", async () => {
    const answer = { kind: "unheard_of", message: "x" };
    vi.mocked(invoke).mockRejectedValue(answer);

    const error = await rejection(bootstrapConnection());

    expect(error.kind).toBeNull();
    expect(error.message).toBe("Tauri command `ensure_daemon_running` failed");
    expect(error.detail).toBe(answer);
  });
});
