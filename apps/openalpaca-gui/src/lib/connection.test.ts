/**
 * How `lib/connection.ts` reads the shell's refusals (Q13): a typed refusal
 * `{ kind, message }` becomes a `ConnectionError` carrying that `kind` and
 * the shell's sentence as it is; Tauri's own refusal (a bare string) keeps
 * its sentence with no `kind`; anything else is named by the command.
 */

import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  vi,
  type MockInstance,
} from "vitest";

import daemonPaths from "./daemon-path.json";
import shellErrors from "./shell-errors.json";

import {
  CONNECTION_CHECK_EVERY_MS,
  CONNECTION_REREAD_AFTER_MS,
  ConnectionError,
  awaitDaemonStopped,
  bootstrapConnection,
  daemonPathOf,
  getCachedConnection,
  keepConnectionFresh,
  readDaemonLogTail,
  refreshConnection,
  resetConnection,
  setStopIntent,
  subscribeInstanceChange,
  type ConnectionInfo,
} from "./connection";
import { fixtureHost } from "./host/fixture";

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
  fixtureHost!.reset();
  vi.restoreAllMocks();
});

describe("a typed refusal of the shell's", () => {
  it("is a ConnectionError with its kind and its sentence, at boot", async () => {
    const message =
      "The daemon did not start within 5 seconds. Its log (/s/state/logs/daemon.log) ends with:\n\nError: this database is newer than this build";
    fixtureHost!.answer("ensureDaemonRunning", () =>
      Promise.reject({
        kind: "spawn_failed",
        message,
        log_tail: "Error: this database is newer than this build",
      }),
    );
    const asked = vi.spyOn(fixtureHost!, "ensureDaemonRunning");

    const error = await rejection(bootstrapConnection());

    expect(asked).toHaveBeenCalledWith();
    expect(error.kind).toBe("spawn_failed");
    expect(error.message).toBe(message);
  });

  it("is a ConnectionError with its kind and its sentence, on reconnect", async () => {
    fixtureHost!.answer("getConnectionInfo", () =>
      Promise.reject({
        kind: "not_running",
        message: "Discovery not found: daemon may not be running",
      }),
    );
    const asked = vi.spyOn(fixtureHost!, "getConnectionInfo");

    const error = await rejection(refreshConnection());

    expect(asked).toHaveBeenCalledWith();
    expect(error.kind).toBe("not_running");
    expect(error.message).toBe(
      "Discovery not found: daemon may not be running",
    );
  });

  it("names an expired token", async () => {
    fixtureHost!.answer("getConnectionInfo", () =>
      Promise.reject({
        kind: "expired",
        message: "Discovery token has expired",
      }),
    );

    const error = await rejection(refreshConnection());

    expect(error.kind).toBe("expired");
    expect(error.message).toBe("Discovery token has expired");
  });

  it("names a log the shell could not read", async () => {
    fixtureHost!.answer("readDaemonLogTail", () =>
      Promise.reject({
        kind: "log_unreadable",
        message:
          "Could not read /s/state/logs/daemon.log: Is a directory (os error 21)",
      }),
    );
    const asked = vi.spyOn(fixtureHost!, "readDaemonLogTail");

    const error = await rejection(readDaemonLogTail(5));

    expect(asked).toHaveBeenCalledWith(5);
    expect(error.kind).toBe("log_unreadable");
    expect(error.message).toBe(
      "Could not read /s/state/logs/daemon.log: Is a directory (os error 21)",
    );
  });

  it("names a stop the shell could not wait for", async () => {
    fixtureHost!.answer("awaitDaemonStopped", () =>
      Promise.reject({
        kind: "stop_unconfirmed",
        message:
          'Could not wait for the daemon to stop: task 7 panicked with message "x"',
      }),
    );

    const error = await rejection(awaitDaemonStopped());

    expect(error.kind).toBe("stop_unconfirmed");
    expect(error.message).toBe(
      'Could not wait for the daemon to stop: task 7 panicked with message "x"',
    );
  });
});

describe("a refusal that is not the shell's", () => {
  it("keeps Tauri's own sentence, a bare string, with no kind", async () => {
    fixtureHost!.answer("readDaemonLogTail", () =>
      Promise.reject("command read_daemon_log_tail missing required key lines"),
    );

    const error = await rejection(readDaemonLogTail(5));

    expect(error.kind).toBeNull();
    expect(error.message).toBe(
      "command read_daemon_log_tail missing required key lines",
    );
  });

  it("is named by its command when the shell's answer is unknown", async () => {
    const answer = { kind: "unheard_of", message: "x" };
    fixtureHost!.answer("ensureDaemonRunning", () => Promise.reject(answer));

    const error = await rejection(bootstrapConnection());

    expect(error.kind).toBeNull();
    expect(error.message).toBe("Tauri command `ensure_daemon_running` failed");
    expect(error.detail).toBe(answer);
  });
});

/**
 * The connection's `daemonPath` (Q21): the shell's table, `daemon-path.json`,
 * read as it is; `null` for a daemon this app did not spawn and for anything
 * that is not one of the shell's.
 */
describe("the daemon's PATH on the connection", () => {
  const info = {
    baseUrl: "http://127.0.0.1:51823",
    token: "t",
    instanceId: "i-1",
  };

  /** Q04's three fields, as an older shell leaves them: absent. */
  const older = { version: null, protocol: null, note: null };

  it("carries each row of the shell's table as it is, at boot and on reconnect", async () => {
    for (const row of Object.values(daemonPaths)) {
      resetConnection();
      fixtureHost!.answerConnection({ ...info, daemonPath: row });
      expect(await bootstrapConnection()).toEqual({
        ...info,
        ...older,
        daemonPath: row,
      });
      expect(await refreshConnection()).toEqual({
        ...info,
        ...older,
        daemonPath: row,
      });
    }
  });

  it("is null for a daemon the app did not spawn", async () => {
    fixtureHost!.answerConnection({ ...info, daemonPath: null });
    expect(await bootstrapConnection()).toEqual({
      ...info,
      ...older,
      daemonPath: null,
    });
  });

  it("is null for anything that is not one of the shell's", () => {
    for (const value of [
      undefined,
      "login-shell",
      { source: "inherited", captureMs: 1, reason: null, passedKeys: [] },
      { source: "login-shell", captureMs: "1", reason: null, passedKeys: [] },
      { source: "login-shell", captureMs: 1, reason: 7, passedKeys: [] },
      {
        source: "fallback-list",
        captureMs: Number.NaN,
        reason: "x",
        passedKeys: [],
      },
      // N-DAEMON-2: the keys passed beside PATH, by name.
      { source: "login-shell", captureMs: 1, reason: null },
      { source: "login-shell", captureMs: 1, reason: null, passedKeys: "A" },
      { source: "login-shell", captureMs: 1, reason: null, passedKeys: [7] },
    ]) {
      expect(daemonPathOf(value)).toBeNull();
    }
    expect(daemonPathOf(daemonPaths.fallback_list)).toEqual(
      daemonPaths.fallback_list,
    );
    // CAPTURE-RETRY: the login shell's, read again without -i.
    expect(daemonPathOf(daemonPaths.login_shell_noninteractive)).toEqual({
      source: "login-shell-noninteractive",
      captureMs: 1840,
      reason: null,
      passedKeys: [],
    });
    expect(daemonPathOf(daemonPaths.login_shell_with_keys)).toEqual({
      source: "login-shell",
      captureMs: 212,
      reason: null,
      passedKeys: ["AWS_PROFILE", "KUBECONFIG"],
    });
  });
});

/**
 * Q04's additive fields: the daemon's `version` and `protocol` and the
 * shell's `note` on a daemon of an older protocol or another version, each
 * `null` when the shell does not send it (an older shell).
 */
describe("the daemon's version, protocol and note on the connection (Q04)", () => {
  const info = {
    baseUrl: "http://127.0.0.1:51823",
    token: "t",
    instanceId: "i-1",
    daemonPath: null,
  };
  const note =
    "This daemon is version 0.1.0 and this app is 0.2.0; they speak the same protocol, so the app uses it. To run this app's own version, use Stop daemon…, then Start daemon.";

  it("carries them at boot and on reconnect", async () => {
    fixtureHost!.answerConnection({
      ...info,
      version: "0.1.0",
      protocol: 1,
      note,
    });
    const expected = { ...info, version: "0.1.0", protocol: 1, note };
    expect(await bootstrapConnection()).toEqual(expected);
    expect(await refreshConnection()).toEqual(expected);
  });

  it("reads them as null from an older shell, or when they are not the shell's", async () => {
    fixtureHost!.answerConnection(info);
    expect(await bootstrapConnection()).toEqual({
      ...info,
      version: null,
      protocol: null,
      note: null,
    });
    resetConnection();
    fixtureHost!.answerConnection({
      ...info,
      version: 1,
      protocol: "1",
      note: 7,
    });
    expect(await bootstrapConnection()).toEqual({
      ...info,
      version: null,
      protocol: null,
      note: null,
    });
  });
});

describe("the shell's Q04 refusals at boot", () => {
  it("names a mismatch, its sentence as it is", async () => {
    const row = shellErrors.refusals.find((r) => r.kind === "mismatch");
    fixtureHost!.answer("ensureDaemonRunning", () => Promise.reject(row));
    const error = await rejection(bootstrapConnection());
    expect(error.kind).toBe("mismatch");
    expect(error.message).toBe(row?.message);
  });

  it("names a daemon still starting, and one past the budget", async () => {
    for (const kind of ["starting", "still_starting"] as const) {
      const row = shellErrors.refusals.find((r) => r.kind === kind);
      fixtureHost!.answer("ensureDaemonRunning", () => Promise.reject(row));
      const error = await rejection(bootstrapConnection());
      expect(error.kind).toBe(kind);
      expect(error.message).toBe(row?.message);
    }
  });
});

/**
 * A daemon restart seen on reconnect is announced to the instance listeners
 * once, whatever the re-bootstrap meets first (`useConnection` clears the
 * query cache on it: every cached id was the old daemon's). Since Q04 the
 * shell answers `starting` at once for a discovery pid that is a live
 * `openalpacad` whose health is silent — a restarted daemon writes
 * `discovery.json` before it serves — so the re-bootstrap the read starts
 * can fail before the new daemon answers, and the window asks again through
 * a boot of its own (`lib/events.ts`, `parkOrWait`) or a ladder's rung. The
 * daemon it left is kept until a read lands, so that read announces the
 * change.
 */
describe("a daemon restart seen on reconnect", () => {
  const info = (instanceId: string): ConnectionInfo => ({
    baseUrl: "http://127.0.0.1:51823",
    token: "t",
    instanceId,
    daemonPath: null,
  });
  let changes: string[] = [];
  let unsubscribe: (() => void) | null = null;

  /** The shell answering each command from its own queue, in order. */
  function shell(answers: Record<string, Array<ConnectionInfo | Error>>) {
    const answer = (command: string) => () => {
      const next = answers[command]?.shift();
      if (next === undefined) {
        return Promise.reject(new Error(`unexpected ${command}`));
      }
      if (next instanceof Error) {
        return Promise.reject(
          shellErrors.refusals.find((r) => r.kind === next.message),
        );
      }
      return Promise.resolve(next);
    };
    fixtureHost!.answer("ensureDaemonRunning", answer("ensure_daemon_running"));
    fixtureHost!.answer("getConnectionInfo", answer("get_connection_info"));
  }
  const starting = () => new Error("starting");

  async function bootedOn(instanceId: string): Promise<void> {
    shell({ ensure_daemon_running: [info(instanceId)] });
    await bootstrapConnection();
    unsubscribe = subscribeInstanceChange((next, previous) => {
      changes.push(`${previous.instanceId}->${next.instanceId}`);
    });
  }

  beforeEach(() => {
    changes = [];
  });

  afterEach(() => {
    unsubscribe?.();
    unsubscribe = null;
  });

  it("is announced once when the re-bootstrap finds the new daemon at once", async () => {
    await bootedOn("A");
    shell({
      get_connection_info: [info("B")],
      ensure_daemon_running: [info("B")],
    });

    expect((await refreshConnection()).instanceId).toBe("B");
    expect(changes).toEqual(["A->B"]);
  });

  it("is announced when the re-bootstrap met it starting and the next boot finds it", async () => {
    await bootedOn("A");
    shell({
      get_connection_info: [info("B")],
      ensure_daemon_running: [starting(), info("B")],
    });

    expect((await rejection(refreshConnection())).kind).toBe("starting");
    expect(getCachedConnection()).toBeNull();
    expect(changes).toEqual([]);

    expect((await bootstrapConnection()).instanceId).toBe("B");
    expect(changes).toEqual(["A->B"]);
  });

  it("is announced when the re-bootstrap met it starting and a later rung finds it", async () => {
    await bootedOn("A");
    shell({
      get_connection_info: [info("B"), info("B")],
      ensure_daemon_running: [starting(), info("B")],
    });

    expect((await rejection(refreshConnection())).kind).toBe("starting");
    expect((await refreshConnection()).instanceId).toBe("B");
    expect(changes).toEqual(["A->B"]);
  });

  it("is not announced again for the same daemon, and a reset forgets a change not yet announced", async () => {
    await bootedOn("A");
    shell({
      get_connection_info: [info("B"), info("B")],
      ensure_daemon_running: [starting(), info("B"), info("B")],
    });
    await rejection(refreshConnection());
    await bootstrapConnection();
    await refreshConnection();
    await bootstrapConnection();
    expect(changes).toEqual(["A->B"]);

    shell({
      get_connection_info: [info("C")],
      ensure_daemon_running: [starting(), info("C")],
    });
    await rejection(refreshConnection());
    resetConnection();
    await bootstrapConnection();
    expect(changes).toEqual(["A->B"]);
  });
});

/**
 * TOKEN-REREAD (`tasks/pending-decisions.md` §1 _Rev 93_ (b)): the daemon
 * renews its token twelve hours after its issue and honours the one it
 * replaced until that one's `expires_at`, so a token the window read is good
 * for at least twelve hours. The window reads the connection again before its
 * cache is that old, judged on the wall clock: a Mac's sleep stops the
 * webview's timers but not `Date.now()`, so a timer of hours alone would let
 * a night's sleep carry a lapsed token into a content URL, the chat stream's
 * `EventSource` or the events socket.
 *
 * Only the timers are faked here; the wall clock is held still and moved by
 * hand, so a look that finds the clock unmoved is told apart from one that
 * finds it moved, which a fake `Date` advancing with the timers cannot show.
 */
describe("the connection read again on the wall clock (TOKEN-REREAD)", () => {
  const HOUR = 60 * 60 * 1000;
  let wall = 0;
  let stop: (() => void) | null = null;
  let boots: MockInstance<() => Promise<ConnectionInfo>>;
  let reads: MockInstance<() => Promise<ConnectionInfo>>;

  /** The shell's answer from now on: `token`, the same daemon. */
  function serving(token: string): void {
    fixtureHost!.answerConnection({
      baseUrl: "http://127.0.0.1:51823",
      token,
      instanceId: "i-1",
      daemonPath: null,
    });
  }

  /** How many times the window read the connection again. */
  function rereads(): number {
    return reads.mock.calls.length;
  }

  beforeEach(async () => {
    vi.useFakeTimers({
      toFake: ["setInterval", "clearInterval", "setTimeout", "clearTimeout"],
    });
    wall = Date.UTC(2026, 10, 16, 12);
    vi.spyOn(Date, "now").mockImplementation(() => wall);
    setStopIntent(null);
    serving("t1");
    boots = vi.spyOn(fixtureHost!, "ensureDaemonRunning");
    reads = vi.spyOn(fixtureHost!, "getConnectionInfo");
    await bootstrapConnection();
    stop = keepConnectionFresh();
  });

  afterEach(() => {
    stop?.();
    stop = null;
    setStopIntent(null);
    vi.useRealTimers();
    vi.mocked(Date.now).mockRestore();
  });

  it("is due well inside the token's twelve hours", () => {
    expect(CONNECTION_REREAD_AFTER_MS + CONNECTION_CHECK_EVERY_MS).toBeLessThan(
      12 * HOUR,
    );
  });

  it("reads it at the next look once the wall clock moved past twelve hours, a sleep no timer counted", async () => {
    wall += 13 * HOUR;
    serving("t2");

    await vi.advanceTimersByTimeAsync(CONNECTION_CHECK_EVERY_MS);

    expect(rereads()).toBe(1);
    expect(getCachedConnection()?.token).toBe("t2");
  });

  it("does not read it while the wall clock stays inside the bound, however many looks", async () => {
    await vi.advanceTimersByTimeAsync(5 * HOUR);
    expect(rereads()).toBe(0);

    wall += CONNECTION_REREAD_AFTER_MS - 1;
    await vi.advanceTimersByTimeAsync(CONNECTION_CHECK_EVERY_MS);
    expect(rereads()).toBe(0);

    wall += 1;
    serving("t2");
    await vi.advanceTimersByTimeAsync(CONNECTION_CHECK_EVERY_MS);
    expect(rereads()).toBe(1);
    expect(getCachedConnection()?.token).toBe("t2");

    // The read is the new stamp: the next is due a whole bound later.
    await vi.advanceTimersByTimeAsync(CONNECTION_CHECK_EVERY_MS);
    expect(rereads()).toBe(1);
  });

  it("reads it when the wall clock was set back", async () => {
    wall -= HOUR;
    await vi.advanceTimersByTimeAsync(CONNECTION_CHECK_EVERY_MS);
    expect(rereads()).toBe(1);
  });

  it("reads a stale one at once when the window is shown or focused again", async () => {
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("focus"));
    await vi.advanceTimersByTimeAsync(0);
    expect(rereads()).toBe(0);

    wall += 13 * HOUR;
    serving("t2");
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(0);
    expect(rereads()).toBe(1);
    expect(getCachedConnection()?.token).toBe("t2");

    wall += 13 * HOUR;
    window.dispatchEvent(new Event("focus"));
    await vi.advanceTimersByTimeAsync(0);
    expect(rereads()).toBe(2);
  });

  it("does not read a stale one when the window is hidden, and does once it is shown", async () => {
    const visibility = vi
      .spyOn(document, "visibilityState", "get")
      .mockReturnValue("hidden");
    wall += 13 * HOUR;
    serving("t2");
    try {
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.advanceTimersByTimeAsync(0);
      expect(rereads()).toBe(0);

      visibility.mockReturnValue("visible");
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.advanceTimersByTimeAsync(0);
      expect(rereads()).toBe(1);
      expect(getCachedConnection()?.token).toBe("t2");
    } finally {
      visibility.mockRestore();
    }
  });

  it("does not read it while the window has no daemon by intent", async () => {
    setStopIntent("stopped_here");
    wall += 13 * HOUR;
    await vi.advanceTimersByTimeAsync(CONNECTION_CHECK_EVERY_MS);
    expect(rereads()).toBe(0);
    // Nor when the daemon was stopped elsewhere (the CLI, another window).
    setStopIntent("stopped_elsewhere");
    await vi.advanceTimersByTimeAsync(CONNECTION_CHECK_EVERY_MS);
    expect(rereads()).toBe(0);
    // Nothing started one either: the boot's call is the only one.
    expect(boots.mock.calls).toHaveLength(1);
  });

  it("does not read it while a boot is in flight", async () => {
    let finish: (answer: unknown) => void = () => undefined;
    const pending = new Promise((resolve) => {
      finish = resolve;
    });
    fixtureHost!.answer("ensureDaemonRunning", () => pending);
    fixtureHost!.answer("getConnectionInfo", () =>
      Promise.resolve({
        baseUrl: "http://127.0.0.1:51823",
        token: "t2",
        instanceId: "i-1",
        daemonPath: null,
      }),
    );
    const boot = bootstrapConnection();
    wall += 13 * HOUR;

    let looked: number;
    try {
      await vi.advanceTimersByTimeAsync(CONNECTION_CHECK_EVERY_MS);
      looked = rereads();
    } finally {
      // Settled whatever the look did, so no boot is left in flight for the
      // next test's.
      finish({
        baseUrl: "http://127.0.0.1:51823",
        token: "t3",
        instanceId: "i-1",
        daemonPath: null,
      });
      await boot;
    }
    expect(looked).toBe(0);
    expect(getCachedConnection()?.token).toBe("t3");
  });

  it("does not read an empty cache: the first read is the boot's", async () => {
    resetConnection();
    wall += 13 * HOUR;
    await vi.advanceTimersByTimeAsync(CONNECTION_CHECK_EVERY_MS);
    expect(rereads()).toBe(0);
  });

  it("keeps the cache when a read fails and tries again at the next look", async () => {
    const notRunning = () =>
      Promise.reject({
        kind: "not_running",
        message: "Discovery not found: daemon may not be running",
      });
    fixtureHost!.answer("ensureDaemonRunning", notRunning);
    fixtureHost!.answer("getConnectionInfo", notRunning);
    wall += 13 * HOUR;

    await vi.advanceTimersByTimeAsync(CONNECTION_CHECK_EVERY_MS);
    expect(rereads()).toBe(1);
    expect(getCachedConnection()?.token).toBe("t1");

    serving("t3");
    await vi.advanceTimersByTimeAsync(CONNECTION_CHECK_EVERY_MS);
    expect(rereads()).toBe(2);
    expect(getCachedConnection()?.token).toBe("t3");
  });

  it("leaves nothing behind once stopped, and a second start holds one look", async () => {
    stop?.();
    stop = null;
    expect(vi.getTimerCount()).toBe(0);

    wall += 13 * HOUR;
    await vi.advanceTimersByTimeAsync(5 * CONNECTION_CHECK_EVERY_MS);
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("focus"));
    await vi.advanceTimersByTimeAsync(0);
    expect(rereads()).toBe(0);

    // StrictMode's mount, cleanup, mount: the second start is the only one.
    stop = keepConnectionFresh();
    expect(vi.getTimerCount()).toBe(1);
  });
});
