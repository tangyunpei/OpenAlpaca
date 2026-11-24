/**
 * A query mounted while the window's first daemon starts.
 *
 * Since Q04-VERDICT the shell answers `starting` at once after the app's own
 * spawn, as it does at the first look, so a query mounted at boot is refused
 * `starting` while the daemon boots. While the window waits on `starting`
 * the query keeps asking, a second apart (`shouldRetry`, `retryDelay`;
 * VIEW-STARTING), so it shows loading, not an error, and loads once the
 * daemon serves. Past the shell's 45 s budget the answer is
 * `still_starting`, which is retried as any failure is, so the query goes to
 * error beside the card; the window's socket, opened by the ladder's rung,
 * must then refetch what failed before it (`843b269f`, the backstop), or a
 * query with no interval of its own stays in error until something else
 * refetches it. Wired as `QueryProvider` wires it: the real connection,
 * `apiRequest`, the app's query client and the resync bridge; only the
 * shell's commands (on FixtureHost), `fetch` and the socket are doubles.
 */

import { QueryObserver } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  bootstrapConnection,
  refreshConnection,
  resetConnection,
  setStopIntent,
  type ConnectionInfo,
} from "./connection";
import {
  DaemonEventsClient,
  STARTING_RETRY_MS,
  type SocketLike,
} from "./events";
import { fixtureHost } from "./host/fixture";
import { apiRequest } from "./http";
import { createQueryClient, invalidateAfterResync } from "./query-client";
import shellErrors from "./shell-errors.json";

const INFO: ConnectionInfo = {
  baseUrl: "http://127.0.0.1:51823",
  token: "t",
  instanceId: "7f3a91c4-0000-4000-8000-000000000000",
  daemonPath: null,
};

class FakeSocket implements SocketLike {
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  close(): void {}
}

function refusal(kind: string): unknown {
  const found = shellErrors.refusals.find((r) => r.kind === kind);
  if (!found) throw new Error(`no ${kind} refusal in shell-errors.json`);
  return found;
}

/**
 * How the shell answers while the daemon boots: what `ensure_daemon_running`
 * says before the daemon serves, and whether `get_connection_info` (the
 * ladder's rung) already reads its file.
 */
interface Boot {
  /** The shell's answer to an ask while the daemon boots. */
  ensureWhileBooting: (elapsedMs: number) => unknown;
  /** How long the daemon boots before it serves. */
  bootMs: number;
  /** When, during the boot, to read the view's status. */
  probeAtMs?: number[];
}

afterEach(() => {
  setStopIntent(null);
  vi.useRealTimers();
  vi.unstubAllGlobals();
  fixtureHost!.reset();
  resetConnection();
});

async function coldStart(boot: Boot) {
  vi.useFakeTimers();
  resetConnection();
  const started = Date.now();
  let serving = false;
  fixtureHost!.answer("ensureDaemonRunning", () => {
    if (serving) return Promise.resolve(INFO);
    return Promise.reject(boot.ensureWhileBooting(Date.now() - started));
  });
  // The file names the booting daemon: the rung reads it and dials.
  fixtureHost!.answer("getConnectionInfo", () => Promise.resolve(INFO));
  const fetchMock = vi.fn(async () => {
    if (!serving) throw new TypeError("Failed to fetch");
    return new Response("[]", {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  });
  vi.stubGlobal("fetch", fetchMock);

  const sockets: FakeSocket[] = [];
  const client = createQueryClient();
  const events = new DaemonEventsClient({
    bootstrap: bootstrapConnection,
    refresh: refreshConnection,
    createSocket: () => {
      const socket = new FakeSocket();
      sockets.push(socket);
      // A socket dialled before the daemon serves fails before it opens.
      if (!serving) {
        queueMicrotask(() => socket.onclose?.({}));
      }
      return socket;
    },
    random: () => 0.5,
    now: () => Date.now(),
    ringSize: 3,
  });
  const offResync = events.onResync(() => invalidateAfterResync(client));

  const observer = new QueryObserver(client, {
    queryKey: ["conversations"],
    queryFn: async () => (await apiRequest("/v1/conversations")).json(),
  });
  // Every status the view is given, and when, from the mount on.
  const seen: { atMs: number; status: string }[] = [];
  const unsubscribe = observer.subscribe((result) => {
    seen.push({ atMs: Date.now() - started, status: result.status });
  });
  void events.connect();

  const probes: { atMs: number; status: string }[] = [];
  for (const atMs of boot.probeAtMs ?? []) {
    await vi.advanceTimersByTimeAsync(atMs - (Date.now() - started));
    probes.push({ atMs, status: observer.getCurrentResult().status });
  }
  await vi.advanceTimersByTimeAsync(boot.bootMs - (Date.now() - started));
  const duringBoot = observer.getCurrentResult().status;
  serving = true;
  const dialledBefore = sockets.length;
  // The window's next ask, or the ladder's next rung, dials the daemon now
  // serving; its socket opens.
  for (let waited = 0; waited < 60_000; waited += 500) {
    await vi.advanceTimersByTimeAsync(500);
    if (sockets.length > dialledBefore) {
      sockets.at(-1)?.onopen?.({});
      break;
    }
  }
  await vi.advanceTimersByTimeAsync(10_000);
  const result = {
    duringBoot,
    probes,
    seen,
    firstErrorAtMs: seen.find((s) => s.status === "error")?.atMs ?? null,
    after: observer.getCurrentResult().status,
    socket: events.getStatus(),
    fetches: fetchMock.mock.calls.length,
  };
  unsubscribe();
  offResync();
  events.disconnect();
  return result;
}

describe("a query mounted while the window's first daemon starts", () => {
  it("keeps loading while the shell answers `starting`, then loads", async () => {
    const result = await coldStart({
      ensureWhileBooting: () => refusal("starting"),
      bootMs: 6_000,
      probeAtMs: [3_500, 5_000],
    });
    // Past the two retries of any other failure (about 3 s), still loading.
    expect(result.probes).toEqual([
      { atMs: 3_500, status: "pending" },
      { atMs: 5_000, status: "pending" },
    ]);
    expect(result.duringBoot).toBe("pending");
    // Never in error, from the mount to the load.
    expect(result.firstErrorAtMs).toBeNull();
    expect(result.socket).toBe("connected");
    expect(result.after).toBe("success");
    expect(result.fetches).toBeGreaterThanOrEqual(1);
  });

  it("goes to error past the budget and loads once the ladder's socket opens", async () => {
    // Past the shell's budget the answer is `still_starting`: the window
    // climbs the reconnect ladder, whose rung reads the file and dials.
    const result = await coldStart({
      ensureWhileBooting: (elapsedMs) =>
        elapsedMs < 45_000 ? refusal("starting") : refusal("still_starting"),
      bootMs: 50_000,
      probeAtMs: [40_000],
    });
    // Loading while the shell answers `starting`...
    expect(result.probes).toEqual([{ atMs: 40_000, status: "pending" }]);
    // ...in error past the budget, beside the card: the query has failed
    // far more than twice by then, so its first failure that is not a
    // `starting` wait ends it (here a `still_starting`, or a failed fetch
    // once the rung has read the file; the rule on `still_starting` alone
    // is pinned below).
    expect(result.firstErrorAtMs).not.toBeNull();
    expect(result.firstErrorAtMs).toBeGreaterThanOrEqual(45_000);
    expect(result.duringBoot).toBe("error");
    // The ladder's socket opens: the resync refetches it (`843b269f`).
    expect(result.socket).toBe("connected");
    expect(result.after).toBe("success");
    expect(result.fetches).toBeGreaterThanOrEqual(1);
  });
});

/**
 * A query mounted while the shell refuses every ask, with no window asking
 * beside it: how often, how far apart and for how long the query asks
 * again on each refusal.
 */
async function refusedFor(
  answer: (elapsedMs: number) => unknown,
  forMs: number,
  options: { stopAtMs?: number } = {},
) {
  vi.useFakeTimers();
  resetConnection();
  const started = Date.now();
  const asks: number[] = [];
  fixtureHost!.answer("ensureDaemonRunning", () => {
    asks.push(Date.now() - started);
    return Promise.reject(answer(Date.now() - started));
  });
  const notRunning = () => Promise.reject(refusal("not_running"));
  fixtureHost!.answer("getConnectionInfo", notRunning);
  fixtureHost!.answer("readDaemonLogTail", notRunning);
  fixtureHost!.answer("awaitDaemonStopped", notRunning);
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => {
      throw new TypeError("Failed to fetch");
    }),
  );
  const client = createQueryClient();
  const observer = new QueryObserver(client, {
    queryKey: ["conversations"],
    queryFn: async () => (await apiRequest("/v1/conversations")).json(),
  });
  const unsubscribe = observer.subscribe(() => {});
  if (options.stopAtMs !== undefined) {
    await vi.advanceTimersByTimeAsync(options.stopAtMs);
    setStopIntent("stopped_elsewhere");
    await vi.advanceTimersByTimeAsync(forMs - options.stopAtMs);
  } else {
    await vi.advanceTimersByTimeAsync(forMs);
  }
  const result = {
    status: observer.getCurrentResult().status,
    asks: [...asks],
  };
  // Long after: nothing asks again.
  await vi.advanceTimersByTimeAsync(120_000);
  const asksLater = asks.length;
  unsubscribe();
  client.clear();
  return { ...result, asksLater };
}

function gaps(asks: number[]): number[] {
  return asks.slice(1).map((at, i) => at - (asks[i] ?? at));
}

describe("how a query refused by the shell asks again", () => {
  it("asks again on `starting` past the two retries, a capped delay apart", async () => {
    const result = await refusedFor(() => refusal("starting"), 40_000);
    expect(result.status).toBe("pending");
    // TanStack's own delay doubles to 30 s: six asks in 40 s. Capped at the
    // window's own cadence, about one a second.
    expect(result.asks.length).toBeGreaterThanOrEqual(30);
    // Every ask the window's own cadence after the last: never longer, and
    // never shorter (a shorter one only asks the shell more).
    expect(new Set(gaps(result.asks))).toEqual(new Set([STARTING_RETRY_MS]));
  });

  it("stops asking on `starting` once the window has no daemon by intent", async () => {
    const result = await refusedFor(() => refusal("starting"), 10_000, {
      stopAtMs: 5_000,
    });
    expect(result.status).toBe("error");
    expect(result.asks.every((at) => at <= 6_000)).toBe(true);
    expect(result.asksLater).toBe(result.asks.length);
  });

  for (const kind of ["still_starting", "mismatch", "spawn_failed"]) {
    it(`gives up on \`${kind}\` after the two retries, never retried forever`, async () => {
      const result = await refusedFor(() => refusal(kind), 60_000);
      expect(result.status).toBe("error");
      // The ask and its two retries, TanStack's doubling delay apart
      // (`defaultRetryDelay` of 0, then 1), not the `starting` cap.
      expect(result.asks).toHaveLength(3);
      expect(gaps(result.asks)).toEqual([1_000, 2_000]);
      expect(result.asksLater).toBe(3);
    });
  }
});
