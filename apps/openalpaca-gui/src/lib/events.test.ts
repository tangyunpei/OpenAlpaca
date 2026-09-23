import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  BACKOFF_BASE_MS,
  BACKOFF_MAX_MS,
  DaemonEventsClient,
  jitteredDelay,
  nextBackoff,
  type ResyncSignal,
  type ServerEvent,
  type ServerEventType,
  type SocketLike,
} from "./events";
import {
  getStopIntent,
  setStopIntent,
  type ConnectionInfo,
} from "./connection";

const INFO: ConnectionInfo = {
  baseUrl: "http://127.0.0.1:51823",
  token: "tok en/+",
  instanceId: "7f3a91c4-0000-4000-8000-000000000000",
};

class FakeSocket implements SocketLike {
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  closed = false;

  constructor(readonly url: string) {}

  close(): void {
    this.closed = true;
  }
}

describe("backoff", () => {
  it("stays inside ±20% of the current backoff", () => {
    expect(jitteredDelay(1000, () => 0)).toBe(800);
    expect(jitteredDelay(1000, () => 0.999999)).toBeCloseTo(1200, 2);
    expect(jitteredDelay(1000, () => 0.5)).toBe(1000);
  });

  it("clamps the jittered delay at the 30 s ceiling", () => {
    expect(jitteredDelay(BACKOFF_MAX_MS, () => 0.999999)).toBe(BACKOFF_MAX_MS);
  });

  it("doubles up to the ceiling and stops", () => {
    expect(nextBackoff(BACKOFF_BASE_MS)).toBe(2000);
    expect(nextBackoff(20_000)).toBe(BACKOFF_MAX_MS);
    expect(nextBackoff(BACKOFF_MAX_MS)).toBe(BACKOFF_MAX_MS);
  });
});

describe("DaemonEventsClient", () => {
  let sockets: FakeSocket[];

  function makeClient(
    overrides: Partial<
      ConstructorParameters<typeof DaemonEventsClient>[0]
    > = {},
  ) {
    return new DaemonEventsClient({
      bootstrap: () => Promise.resolve(INFO),
      refresh: () => Promise.resolve(INFO),
      createSocket: (url) => {
        const socket = new FakeSocket(url);
        sockets.push(socket);
        return socket;
      },
      random: () => 0.5,
      now: () => Date.now(),
      ringSize: 3,
      ...overrides,
    });
  }

  function latest(): FakeSocket {
    const socket = sockets.at(-1);
    if (!socket) throw new Error("no socket was created");
    return socket;
  }

  /**
   * A `bootstrap` that hangs until `release()`, handing every caller the same
   * promise — `connection.ts`'s own `bootstrapInFlight` does exactly that, and
   * it is why two overlapping `connect()` continuations run back to back.
   */
  function sharedBootstrap(): {
    bootstrap: () => Promise<ConnectionInfo>;
    release: (info: ConnectionInfo) => void;
  } {
    let release!: (info: ConnectionInfo) => void;
    const pending = new Promise<ConnectionInfo>((resolve) => {
      release = resolve;
    });
    return { bootstrap: () => pending, release };
  }

  beforeEach(() => {
    sockets = [];
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("puts the token in the query string because WS headers are impossible", async () => {
    const client = makeClient();
    await client.connect();

    expect(latest().url).toBe(
      "ws://127.0.0.1:51823/v1/events?token=tok%20en%2F%2B",
    );
    client.disconnect();
  });

  it("tags events with a monotonic id and keeps a bounded newest-first ring", async () => {
    const client = makeClient();
    const seen: ServerEvent[] = [];
    client.onEvent((event) => seen.push(event));
    await client.connect();

    const socket = latest();
    socket.onopen?.({});
    for (const id of ["a", "b", "c", "d"]) {
      socket.onmessage?.({
        data: JSON.stringify({
          type: "connector_status",
          id,
          status: "running",
        }),
      });
    }

    expect(seen.map((e) => e._id)).toEqual([0, 1, 2, 3]);
    expect(client.getEvents()).toHaveLength(3);
    expect(client.getEvents()[0]).toMatchObject({ id: "d" });
    client.disconnect();
  });

  // T28. Narrowing on `type` is what proves the union member exists with the
  // right field types — `bun run check` fails if any of these reads is wrong.
  it("carries an artifact_written frame through with its own fields", async () => {
    const client = makeClient();
    const seen: ServerEvent[] = [];
    client.onEvent((event) => seen.push(event));
    await client.connect();

    const socket = latest();
    socket.onopen?.({});
    socket.onmessage?.({
      data: JSON.stringify({
        type: "artifact_written",
        artifact_id: "a-1",
        task_id: "b41",
        agent_id: "writing_agent",
        name: "01-quarterly-report.md",
        kind: "markdown",
        version: 2,
        path: "/p/.openalpaca/artifacts/run/01-quarterly-report.md",
        ts: "2026-09-05T10:00:00Z",
        instance_id: "7f3a",
      }),
    });

    const event = seen[0];
    if (event?.type !== "artifact_written") {
      throw new Error(`expected artifact_written, got ${event?.type}`);
    }
    expect(event.artifact_id).toBe("a-1");
    expect(event.task_id).toBe("b41");
    expect(event.agent_id).toBe("writing_agent");
    expect(event.name).toBe("01-quarterly-report.md");
    expect(event.kind).toBe("markdown");
    expect(event.version).toBe(2);
    expect(event.path).toBe(
      "/p/.openalpaca/artifacts/run/01-quarterly-report.md",
    );
    client.disconnect();
  });

  it("accepts an artifact_written frame with no run and no agent", async () => {
    const client = makeClient();
    const seen: ServerEvent[] = [];
    client.onEvent((event) => seen.push(event));
    await client.connect();

    const socket = latest();
    socket.onopen?.({});
    socket.onmessage?.({
      data: JSON.stringify({
        type: "artifact_written",
        artifact_id: "a-2",
        task_id: null,
        agent_id: null,
        name: "01-notes.md",
        kind: "markdown",
        version: 1,
        path: "/h/.openalpaca/artifacts/loose/01-notes.md",
        ts: "2026-09-05T10:00:00Z",
        instance_id: "7f3a",
      }),
    });

    const event = seen[0];
    if (event?.type !== "artifact_written") {
      throw new Error(`expected artifact_written, got ${event?.type}`);
    }
    expect(event.task_id).toBeNull();
    expect(event.agent_id).toBeNull();
    client.disconnect();
  });

  // Phase 4. Narrowing on `type` proves the union member exists with the right
  // field types; `bun run check` fails if any of these reads is wrong.
  it("carries a subagent_span open through with its own fields", async () => {
    const client = makeClient();
    const seen: ServerEvent[] = [];
    client.onEvent((event) => seen.push(event));
    await client.connect();

    const socket = latest();
    socket.onopen?.({});
    socket.onmessage?.({
      data: JSON.stringify({
        type: "subagent_span",
        task_id: "b41",
        span_id: "node-1",
        label: "review\u00b71",
        template_id: "review_agent",
        agent_instance_id: "review_agent::a1b2",
        state: "running",
        detail: null,
        started_at: "2026-09-05T10:00:00.000Z",
        ended_at: null,
        duration_ms: null,
        output_preview: null,
        ts: "2026-09-05T10:00:00Z",
        instance_id: "7f3a",
      }),
    });

    const event = seen[0];
    if (event?.type !== "subagent_span") {
      throw new Error(`expected subagent_span, got ${event?.type}`);
    }
    expect(event.task_id).toBe("b41");
    expect(event.span_id).toBe("node-1");
    expect(event.label).toBe("review\u00b71");
    expect(event.template_id).toBe("review_agent");
    expect(event.agent_instance_id).toBe("review_agent::a1b2");
    expect(event.state).toBe("running");
    expect(event.started_at).toBe("2026-09-05T10:00:00.000Z");
    expect(event.ended_at).toBeNull();
    expect(event.duration_ms).toBeNull();
    client.disconnect();
  });

  it("carries a subagent_span close, cancellation and all", async () => {
    const client = makeClient();
    const seen: ServerEvent[] = [];
    client.onEvent((event) => seen.push(event));
    await client.connect();

    const socket = latest();
    socket.onopen?.({});
    socket.onmessage?.({
      data: JSON.stringify({
        type: "subagent_span",
        task_id: "b41",
        span_id: "node-1",
        label: "review\u00b71",
        template_id: "review_agent",
        agent_instance_id: "review_agent::a1b2",
        state: "cancelled",
        detail: "cancelled before starting",
        started_at: "2026-09-05T10:00:00.000Z",
        ended_at: "2026-09-05T10:00:04.500Z",
        duration_ms: 4500,
        output_preview: "partial",
        ts: "2026-09-05T10:00:04Z",
        instance_id: "7f3a",
      }),
    });

    const event = seen[0];
    if (event?.type !== "subagent_span") {
      throw new Error(`expected subagent_span, got ${event?.type}`);
    }
    expect(event.state).toBe("cancelled");
    expect(event.detail).toBe("cancelled before starting");
    expect(event.ended_at).toBe("2026-09-05T10:00:04.500Z");
    expect(event.duration_ms).toBe(4500);
    expect(event.output_preview).toBe("partial");
    client.disconnect();
  });

  it("drops frames that are not tagged ServerEvents", async () => {
    const client = makeClient();
    await client.connect();
    const socket = latest();
    socket.onopen?.({});

    socket.onmessage?.({ data: "{" });
    socket.onmessage?.({ data: JSON.stringify({ no: "type" }) });
    socket.onmessage?.({ data: 42 });

    expect(client.getEvents()).toHaveLength(0);
    client.disconnect();
  });

  it("reconnects on a jittered schedule and doubles the backoff", async () => {
    const refresh = vi.fn(() => Promise.resolve(INFO));
    const client = makeClient({ refresh, random: () => 0 });
    await client.connect();

    latest().onopen?.({});
    expect(client.getStatus()).toBe("connected");

    // First drop: 1000 × 0.8 = 800 ms.
    latest().onclose?.({});
    expect(client.getStatus()).toBe("disconnected");
    await vi.advanceTimersByTimeAsync(799);
    expect(refresh).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(refresh).toHaveBeenCalledTimes(1);

    // Second drop without an intervening open: 2000 × 0.8 = 1600 ms.
    latest().onclose?.({});
    await vi.advanceTimersByTimeAsync(1599);
    expect(refresh).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(refresh).toHaveBeenCalledTimes(2);

    client.disconnect();
  });

  it("resets the backoff once a reconnect succeeds", async () => {
    const refresh = vi.fn(() => Promise.resolve(INFO));
    const client = makeClient({ refresh, random: () => 0 });
    await client.connect();
    latest().onopen?.({});

    latest().onclose?.({});
    await vi.advanceTimersByTimeAsync(800);
    latest().onopen?.({});

    latest().onclose?.({});
    await vi.advanceTimersByTimeAsync(800);
    expect(refresh).toHaveBeenCalledTimes(2);

    client.disconnect();
  });

  it("signals a possible gap in the stream after a reconnect, but not on first connect", async () => {
    const signals: ResyncSignal[] = [];
    const client = makeClient({ random: () => 0 });
    client.onResync((signal) => signals.push(signal));

    await client.connect();
    latest().onopen?.({});
    expect(signals).toHaveLength(0);

    latest().onclose?.({});
    await vi.advanceTimersByTimeAsync(800);
    latest().onopen?.({});

    expect(signals).toHaveLength(1);
    expect(signals[0]?.reason).toBe("reconnected");
    client.disconnect();
  });

  it("signals and clears the ring when the daemon identity changes", async () => {
    const restarted: ConnectionInfo = {
      ...INFO,
      instanceId: "different-instance",
    };
    const signals: ResyncSignal[] = [];
    const client = makeClient({
      refresh: () => Promise.resolve(restarted),
      random: () => 0,
    });
    client.onResync((signal) => signals.push(signal));

    await client.connect();
    latest().onopen?.({});
    latest().onmessage?.({ data: JSON.stringify({ type: "heartbeat" }) });
    expect(client.getEvents()).toHaveLength(1);

    latest().onclose?.({});
    await vi.advanceTimersByTimeAsync(800);

    expect(signals.map((s) => s.reason)).toContain("instance_changed");
    expect(client.getEvents()).toHaveLength(0);
    client.disconnect();
  });

  /**
   * The CLI or another window stopped the daemon. It says so on the socket
   * before it closes it, and a window that did not ask must neither climb a
   * ladder against it nor ever respawn it — it records who stopped it and
   * waits for an explicit Start.
   */
  it("treats a daemon_shutting_down frame it did not ask for as stopped elsewhere, and schedules nothing", async () => {
    setStopIntent(null);
    const refresh = vi.fn(() => Promise.resolve(INFO));
    const bootstrap = vi.fn(() => Promise.resolve(INFO));
    const client = makeClient({ refresh, bootstrap, random: () => 0 });
    const seen: ServerEventType[] = [];
    client.onEvent((event) => seen.push(event.type));
    await client.connect();
    const socket = latest();
    socket.onopen?.({});

    socket.onmessage?.({
      data: JSON.stringify({
        type: "daemon_shutting_down",
        grace_secs: 10,
        ts: "2026-09-22T10:00:00Z",
        instance_id: INFO.instanceId,
      }),
    });
    expect(getStopIntent()).toBe("stopped_elsewhere");
    // Still an ordinary frame for everyone else — the Event log shows it.
    expect(seen).toEqual(["daemon_shutting_down"]);

    socket.onclose?.({});
    expect(client.getStatus()).toBe("disconnected");
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(120_000);
    expect(refresh).not.toHaveBeenCalled();
    expect(bootstrap).toHaveBeenCalledTimes(1);
    setStopIntent(null);
  });

  it("keeps this window's own stop intent when the frame arrives", async () => {
    setStopIntent("stopped_here");
    const client = makeClient();
    await client.connect();
    latest().onmessage?.({
      data: JSON.stringify({
        type: "daemon_shutting_down",
        grace_secs: 10,
        ts: "2026-09-22T10:00:00Z",
        instance_id: INFO.instanceId,
      }),
    });
    expect(getStopIntent()).toBe("stopped_here");
    setStopIntent(null);
    client.disconnect();
  });

  /** `resume()` reads discovery on every rung and never bootstraps. */
  it("resumes the ladder without bootstrapping", async () => {
    const refresh = vi.fn(() => Promise.resolve(INFO));
    const bootstrap = vi.fn(() => Promise.resolve(INFO));
    const client = makeClient({ refresh, bootstrap, random: () => 0.5 });
    await client.connect();
    client.disconnect();
    expect(bootstrap).toHaveBeenCalledTimes(1);

    client.resume();
    await vi.advanceTimersByTimeAsync(1_000);

    expect(refresh).toHaveBeenCalledTimes(1);
    expect(bootstrap).toHaveBeenCalledTimes(1);
    expect(client.getStatus()).toBe("connecting");
    client.disconnect();
  });

  it("stops reconnecting after disconnect(), and nulls handlers before closing", async () => {
    const refresh = vi.fn(() => Promise.resolve(INFO));
    const client = makeClient({ refresh, random: () => 0 });
    await client.connect();
    const socket = latest();
    socket.onopen?.({});

    client.disconnect();

    expect(socket.closed).toBe(true);
    expect(socket.onclose).toBeNull();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(refresh).not.toHaveBeenCalled();
  });

  /**
   * A superseded `connect()` never opens a socket (PR #31 review, finding 4).
   *
   * `connect()`'s continuation was the one `openSocket` call site with no
   * teardown in front of it, so two of them landing back to back left the
   * first socket open, fully wired and unreachable by `teardownSocket()` for
   * ever — every frame delivered twice until the survivor closed. The flag
   * cannot catch it: the second `connect()` sets `reconnectEnabled` back to
   * true before the first continuation runs. Only a generation stamp answers
   * "is this still the current attempt?".
   */
  it("opens one socket when a connect is superseded mid-bootstrap", async () => {
    // `bootstrapConnection` shares one in-flight promise, so overlapping
    // `connect()` calls resolve off the same one and their continuations run
    // back to back, in call order. That is the shape that orphans.
    const { bootstrap, release } = sharedBootstrap();
    const client = makeClient({ bootstrap });

    // StrictMode's mount → cleanup → mount, all while bootstrap is pending.
    void client.connect();
    client.disconnect();
    void client.connect();
    release(INFO);
    await vi.advanceTimersByTimeAsync(0);

    expect(sockets).toHaveLength(1);
    expect(sockets.filter((socket) => !socket.closed)).toHaveLength(1);

    client.disconnect();
  });

  it("opens one socket when two connects overlap with no disconnect", async () => {
    const { bootstrap, release } = sharedBootstrap();
    const client = makeClient({ bootstrap });

    // Two Reconnect clicks during one multi-second sidecar spawn.
    void client.connect();
    void client.connect();
    release(INFO);
    await vi.advanceTimersByTimeAsync(0);

    expect(sockets).toHaveLength(1);
    expect(sockets.filter((socket) => !socket.closed)).toHaveLength(1);

    client.disconnect();
  });

  it("opens no socket at all when the connect was cancelled", async () => {
    const { bootstrap, release } = sharedBootstrap();
    const client = makeClient({ bootstrap });

    void client.connect();
    client.disconnect();
    release(INFO);
    await vi.advanceTimersByTimeAsync(0);

    expect(sockets).toHaveLength(0);
    expect(client.getStatus()).toBe("disconnected");
  });

  /**
   * The backoff ladder's own continuation is stamped too: a `connect()` that
   * started while `refresh()` was in flight re-enables the flag the ladder
   * checks, so the flag alone would let the timer open a second socket over
   * the one `connect()` is about to open.
   */
  it("lets a concurrent connect supersede an in-flight reconnect", async () => {
    let releaseRefresh!: (info: ConnectionInfo) => void;
    const client = makeClient({
      refresh: () =>
        new Promise<ConnectionInfo>((resolve) => {
          releaseRefresh = resolve;
        }),
      random: () => 0,
    });

    await client.connect();
    latest().onopen?.({});
    latest().onclose?.({});
    await vi.advanceTimersByTimeAsync(800);
    expect(sockets).toHaveLength(1);

    // The user hits Reconnect while the ladder's `refresh()` is pending.
    await client.connect();
    expect(sockets).toHaveLength(2);

    releaseRefresh(INFO);
    await vi.advanceTimersByTimeAsync(0);

    expect(sockets).toHaveLength(2);
    expect(sockets.filter((socket) => !socket.closed)).toHaveLength(1);

    client.disconnect();
  });

  it("keeps retrying when the reconnect handshake itself fails", async () => {
    const refresh = vi.fn(() => Promise.reject(new Error("discovery gone")));
    const client = makeClient({ refresh, random: () => 0 });
    await client.connect();
    latest().onopen?.({});
    latest().onclose?.({});

    await vi.advanceTimersByTimeAsync(800);
    expect(refresh).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1600);
    expect(refresh).toHaveBeenCalledTimes(2);

    client.disconnect();
  });

  /**
   * A daemon that wrote `discovery.json` and then died on its database leaves
   * a stale endpoint: the ladder dials it and the socket fails. That failure
   * is not a reason, and it must not replace the one the bootstrap brought
   * back — only a socket that opens retires it.
   */
  it("keeps the bootstrap's reason through a later socket failure, until a socket opens", async () => {
    const reason =
      "The daemon did not start within 5 seconds. Its log ends with:\n\nFATAL: legacy schema v39";
    const client = makeClient({
      bootstrap: () => Promise.reject(new Error(reason)),
      random: () => 0,
    });
    const seen: Array<string | null> = [];
    client.onStatus(() => seen.push(client.getLastError()));

    await client.connect();
    expect(client.getLastError()).toBe(reason);

    // The ladder's first rung reads the stale discovery and dials the port.
    await vi.advanceTimersByTimeAsync(800);
    expect(sockets).toHaveLength(1);
    latest().onerror?.({});
    latest().onclose?.({});

    expect(client.getStatus()).toBe("disconnected");
    expect(client.getLastError()).toBe(reason);
    expect(seen).not.toContain("WebSocket connection error");

    // A daemon that does answer retires it.
    await vi.advanceTimersByTimeAsync(1600);
    latest().onopen?.({});
    expect(client.getStatus()).toBe("connected");
    expect(client.getLastError()).toBeNull();

    // And a transport failure after that is reported as what it is.
    latest().onerror?.({});
    expect(client.getLastError()).toBe("WebSocket connection error");

    client.disconnect();
  });
});

describe("ServerEvent union", () => {
  // P9 (Phase 8): `dag_node_status` was deleted from the wire union — the GUI
  // has rendered `subagent_span` exclusively since T32. If this variant were
  // ever reintroduced, the assignment below would stop failing to type-check
  // and `bun run check` would fail on an unused `@ts-expect-error` directive.
  it("no longer admits dag_node_status", () => {
    // @ts-expect-error "dag_node_status" was removed from ServerEventType (P9)
    const legacy: ServerEventType = "dag_node_status";
    expect(legacy).toBe("dag_node_status");
  });
});
