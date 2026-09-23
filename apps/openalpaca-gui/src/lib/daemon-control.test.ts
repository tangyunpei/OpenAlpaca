/**
 * Stopping and starting the daemon from this window (`lib/daemon-control.ts`).
 *
 * Driven against a real `DaemonEventsClient` on fake sockets and fake timers,
 * so "no ladder climbs" and "nothing respawns the daemon" are observed on the
 * client that would do either — not on a mock of it.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  getStopIntent,
  getStopPhase,
  setStopIntent,
  setStopPhase,
  type ConnectionInfo,
  type DaemonStopReport,
} from "./connection";
import {
  startDaemon,
  stopDaemon,
  stopToast,
  type DaemonControlDeps,
} from "./daemon-control";
import { DaemonEventsClient, type SocketLike } from "./events";
import { ApiError } from "./http";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const INFO: ConnectionInfo = {
  baseUrl: "http://127.0.0.1:51823",
  token: "tok",
  instanceId: "7f3a91c4-0000-4000-8000-000000000000",
};

class FakeSocket implements SocketLike {
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  closed = false;
  close(): void {
    this.closed = true;
  }
}

function harness() {
  const sockets: FakeSocket[] = [];
  const bootstrap = vi.fn(() => Promise.resolve(INFO));
  const refresh = vi.fn(() => Promise.resolve(INFO));
  const client = new DaemonEventsClient({
    bootstrap,
    refresh,
    createSocket: () => {
      const socket = new FakeSocket();
      sockets.push(socket);
      return socket;
    },
    random: () => 0.5,
    now: () => Date.now(),
    ringSize: 10,
  });
  const latest = () => {
    const socket = sockets.at(-1);
    if (!socket) throw new Error("no socket");
    return socket;
  };
  return { client, bootstrap, refresh, latest, sockets };
}

/** A promise the test settles by hand. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function depsFor(
  client: DaemonEventsClient,
  overrides: Partial<DaemonControlDeps> = {},
): DaemonControlDeps {
  return {
    disconnect: () => client.disconnect(),
    connect: () => client.connect(),
    resume: () => client.resume(),
    shutdown: () => Promise.resolve({ status: "shutting_down" }),
    awaitStopped: () =>
      Promise.resolve<DaemonStopReport>({
        outcome: "stopped",
        pid: null,
        waitedMs: 3200,
      }),
    ...overrides,
  };
}

async function connected(h: ReturnType<typeof harness>) {
  await h.client.connect();
  h.latest().onopen?.({});
  expect(h.client.getStatus()).toBe("connected");
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  setStopIntent(null);
  setStopPhase(null);
  vi.useRealTimers();
});

describe("stopDaemon", () => {
  /**
   * The fix, in order: intent, then the socket, then the POST. With the
   * socket already closed by the client itself, the daemon's own close can
   * start nothing — and nothing afterwards brings the daemon back.
   */
  it("disconnects before the POST, and the ladder stays idle throughout", async () => {
    const h = harness();
    await connected(h);
    const socket = h.latest();
    const post = deferred<unknown>();

    const result = stopDaemon(
      depsFor(h.client, { shutdown: () => post.promise }),
    );

    // The POST is in flight: the intent is set, the socket already closed.
    expect(getStopIntent()).toBe("stopped_here");
    expect(socket.closed).toBe(true);
    expect(h.client.getStatus()).toBe("disconnected");
    expect(vi.getTimerCount()).toBe(0);

    post.resolve({ status: "shutting_down" });
    expect(await result).toEqual({ kind: "stopped" });

    await vi.advanceTimersByTimeAsync(120_000);
    expect(vi.getTimerCount()).toBe(0);
    expect(h.refresh).not.toHaveBeenCalled();
    expect(h.bootstrap).toHaveBeenCalledTimes(1); // the first connect only
    expect(getStopIntent()).toBe("stopped_here");
  });

  it("closes the dialog on the POST's response, before the wait resolves", async () => {
    const h = harness();
    await connected(h);
    const wait = deferred<DaemonStopReport>();
    const onPosted = vi.fn();

    const result = stopDaemon(
      depsFor(h.client, { awaitStopped: () => wait.promise }),
      onPosted,
    );
    await vi.advanceTimersByTimeAsync(0);
    expect(onPosted).toHaveBeenCalledTimes(1);

    wait.resolve({ outcome: "not_running", pid: null, waitedMs: 0 });
    expect(await result).toEqual({ kind: "stopped" });
    expect(onPosted).toHaveBeenCalledTimes(1);
  });

  /**
   * The POST's `200` is only an acceptance: the process can hold its lock for
   * up to 15 s more, and a daemon started in that tail loses the lock race.
   * So the window is *stopping* — not stopped — until the wait answers, and
   * then it is whatever the wait said.
   */
  it("is stopping from the POST until the wait answers, then what it answered", async () => {
    const outcomes: Array<[DaemonStopReport["outcome"], string]> = [
      ["stopped", "stopped"],
      ["not_running", "stopped"],
      ["still_alive", "still_alive"],
      ["lock_still_held", "lock_still_held"],
    ];
    for (const [outcome, phase] of outcomes) {
      const h = harness();
      await connected(h);
      const post = deferred<unknown>();
      const wait = deferred<DaemonStopReport>();
      const onPosted = vi.fn();

      const result = stopDaemon(
        depsFor(h.client, {
          shutdown: () => post.promise,
          awaitStopped: () => wait.promise,
        }),
        onPosted,
      );
      expect(getStopPhase()).toBe("stopping");

      post.resolve({ status: "shutting_down" });
      await vi.advanceTimersByTimeAsync(0);
      // The dialog has closed; the process may still be running.
      expect(onPosted).toHaveBeenCalledTimes(1);
      expect(getStopPhase()).toBe("stopping");

      wait.resolve({ outcome, pid: 41287, waitedMs: 3200 });
      await result;
      expect(getStopPhase()).toBe(phase);
      expect(getStopIntent()).toBe("stopped_here");
      setStopIntent(null);
      expect(getStopPhase()).toBeNull();
    }
  });

  it("is unconfirmed, not stopped, when the wait itself fails", async () => {
    const h = harness();
    await connected(h);

    await stopDaemon(
      depsFor(h.client, {
        awaitStopped: () => Promise.reject(new Error("no Tauri bridge")),
      }),
    );

    expect(getStopPhase()).toBe("unconfirmed");
  });

  it("leaves no phase behind when the stop never happened", async () => {
    const refused = harness();
    await connected(refused);
    await stopDaemon(
      depsFor(refused.client, {
        shutdown: () =>
          Promise.reject(new ApiError("forbidden", 403, "FORBIDDEN")),
      }),
    );
    expect(getStopPhase()).toBeNull();

    const unreachable = harness();
    await connected(unreachable);
    await stopDaemon(
      depsFor(unreachable.client, {
        shutdown: () =>
          Promise.reject(new ApiError("Failed to fetch", 0, null)),
      }),
    );
    expect(getStopPhase()).toBeNull();
    unreachable.client.disconnect();
  });

  /** The daemon answered and said no: it is alive, so the window goes back. */
  it("leaves the window connected when the POST is refused", async () => {
    const h = harness();
    await connected(h);

    const result = await stopDaemon(
      depsFor(h.client, {
        shutdown: () =>
          Promise.reject(new ApiError("forbidden", 403, "FORBIDDEN")),
      }),
    );
    h.latest().onopen?.({});

    expect(result).toEqual({ kind: "refused", message: "forbidden" });
    expect(getStopIntent()).toBeNull();
    expect(h.client.getStatus()).toBe("connected");
  });

  /**
   * Nothing answered. Bootstrapping would spawn a daemon — the opposite of
   * what was asked — so the window climbs the read-only ladder instead.
   */
  it("never bootstraps when the POST could not reach the daemon", async () => {
    const h = harness();
    await connected(h);

    const result = await stopDaemon(
      depsFor(h.client, {
        shutdown: () =>
          Promise.reject(new ApiError("Failed to fetch", 0, null)),
      }),
    );

    expect(result).toEqual({
      kind: "unreachable",
      message: "Failed to fetch",
    });
    expect(getStopIntent()).toBeNull();
    expect(h.bootstrap).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1_200);
    expect(h.refresh).toHaveBeenCalledTimes(1);
    expect(h.bootstrap).toHaveBeenCalledTimes(1);
  });

  it("reports each way the wait can end, and never 'stopped' on the 200 alone", async () => {
    const cases: Array<[DaemonStopReport | Error, unknown]> = [
      [
        { outcome: "still_alive", pid: 41287, waitedMs: 15_000 },
        { kind: "still_alive", pid: 41287 },
      ],
      [
        { outcome: "lock_still_held", pid: null, waitedMs: 15_000 },
        { kind: "lock_still_held" },
      ],
      [
        new Error("no Tauri bridge"),
        { kind: "unconfirmed", message: "no Tauri bridge" },
      ],
    ];
    for (const [report, expected] of cases) {
      const h = harness();
      await connected(h);
      const result = await stopDaemon(
        depsFor(h.client, {
          awaitStopped: () =>
            report instanceof Error
              ? Promise.reject(report)
              : Promise.resolve(report),
        }),
      );
      expect(result).toEqual(expected);
      expect(getStopIntent()).toBe("stopped_here");
      setStopIntent(null);
    }
  });
});

describe("startDaemon", () => {
  it("clears the intent and re-bootstraps", async () => {
    const h = harness();
    await connected(h);
    await stopDaemon(depsFor(h.client));
    expect(getStopIntent()).toBe("stopped_here");

    expect(getStopPhase()).toBe("stopped");

    await startDaemon({ connect: () => h.client.connect() });

    expect(getStopIntent()).toBeNull();
    expect(getStopPhase()).toBeNull();
    expect(h.bootstrap).toHaveBeenCalledTimes(2);
    h.latest().onopen?.({});
    expect(h.client.getStatus()).toBe("connected");
  });

  it("clears an intent another client set, too", async () => {
    setStopIntent("stopped_elsewhere");
    const connect = vi.fn(() => Promise.resolve());

    await startDaemon({ connect });

    expect(getStopIntent()).toBeNull();
    expect(connect).toHaveBeenCalledTimes(1);
  });
});

describe("stopToast", () => {
  it("says stopped only when the wait said so, and names the terminal fallback otherwise", () => {
    expect(stopToast({ kind: "stopped" })).toBe("Daemon stopped.");
    expect(stopToast({ kind: "still_alive", pid: 41287 })).toMatch(
      /did not stop within 15 seconds.*openalpaca daemon stop/,
    );
    expect(stopToast({ kind: "lock_still_held" })).toMatch(
      /still holds its lock.*openalpaca daemon status/,
    );
    expect(stopToast({ kind: "unreachable", message: "x" })).toMatch(
      /Could not reach the daemon to stop it.*openalpaca daemon stop/,
    );
    expect(stopToast({ kind: "refused", message: "forbidden" })).toBe(
      "The daemon refused to stop: forbidden",
    );
    expect(stopToast({ kind: "unconfirmed", message: "x" })).not.toMatch(
      /^Daemon stopped/,
    );
  });
});
