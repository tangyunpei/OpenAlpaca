/**
 * A query mounted while the window's first daemon starts.
 *
 * Since Q04-VERDICT the shell answers `starting` at once after the app's own
 * spawn, as it does at the first look, so a query mounted at boot fails its
 * ask and its two retries while the daemon boots (`shouldRetry`). The
 * window's own ask opens the socket once the daemon serves; that first open
 * must refetch what failed before it, or a query with no interval of its own
 * stays in error until something else refetches it. Wired as
 * `QueryProvider` wires it: the real connection, `apiRequest`, the app's
 * query client and the resync bridge; only the shell's `invoke`, `fetch`
 * and the socket are doubles.
 */

import { invoke } from "@tauri-apps/api/core";
import { QueryObserver } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  bootstrapConnection,
  refreshConnection,
  resetConnection,
  type ConnectionInfo,
} from "./connection";
import { DaemonEventsClient, type SocketLike } from "./events";
import { apiRequest } from "./http";
import { createQueryClient, invalidateAfterResync } from "./query-client";
import shellErrors from "./shell-errors.json";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

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
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.mocked(invoke).mockReset();
  resetConnection();
});

async function coldStart(boot: Boot) {
  vi.useFakeTimers();
  resetConnection();
  const started = Date.now();
  let serving = false;
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "ensure_daemon_running") {
      if (serving) return Promise.resolve(INFO);
      return Promise.reject(boot.ensureWhileBooting(Date.now() - started));
    }
    // The file names the booting daemon: the rung reads it and dials.
    if (command === "get_connection_info") return Promise.resolve(INFO);
    return Promise.reject(new Error(`unexpected ${command}`));
  });
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
  const unsubscribe = observer.subscribe(() => {});
  void events.connect();

  await vi.advanceTimersByTimeAsync(boot.bootMs);
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
  it("loads once the first socket opens after a boot answered `starting`", async () => {
    const result = await coldStart({
      ensureWhileBooting: () => refusal("starting"),
      bootMs: 6_000,
    });
    // Its ask and both retries met `starting`: in error while it boots.
    expect(result.duringBoot).toBe("error");
    expect(result.socket).toBe("connected");
    expect(result.after).toBe("success");
    expect(result.fetches).toBeGreaterThanOrEqual(1);
  });

  it("loads once the ladder's socket opens after `still_starting`", async () => {
    // Past the shell's budget the answer is `still_starting`: the window
    // climbs the reconnect ladder, whose rung reads the file and dials.
    const result = await coldStart({
      ensureWhileBooting: (elapsedMs) =>
        elapsedMs < 45_000 ? refusal("starting") : refusal("still_starting"),
      bootMs: 50_000,
    });
    expect(result.duringBoot).toBe("error");
    expect(result.socket).toBe("connected");
    expect(result.after).toBe("success");
    expect(result.fetches).toBeGreaterThanOrEqual(1);
  });
});
