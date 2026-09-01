/**
 * `QueryProvider` is the only thing that connects the socket to the cache, so
 * this asserts the subscription itself: that mounting connects, that a frame
 * arriving on the socket invalidates, that the "possibly missed events" signal
 * invalidates everything, and that unmounting lets go.
 *
 * The client is doubled at the socket boundary (`daemonEvents`), not at the
 * map — `invalidationKeysFor` has its own tests.
 */

import { QueryClient } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { daemonEvents, type ResyncSignal, type ServerEvent } from "./events";
import { QueryProvider } from "./query-provider";
import { qk } from "./query-keys";

interface Captured {
  event: ((event: ServerEvent) => void) | null;
  resync: ((signal: ResyncSignal) => void) | null;
  disposed: number;
}

function captureSocket(): Captured {
  const captured: Captured = { event: null, resync: null, disposed: 0 };
  vi.spyOn(daemonEvents, "connect").mockResolvedValue(undefined);
  vi.spyOn(daemonEvents, "disconnect").mockImplementation(() => {});
  vi.spyOn(daemonEvents, "onEvent").mockImplementation((listener) => {
    captured.event = listener;
    return () => {
      captured.disposed += 1;
    };
  });
  vi.spyOn(daemonEvents, "onResync").mockImplementation((listener) => {
    captured.resync = listener;
    return () => {
      captured.disposed += 1;
    };
  });
  return captured;
}

function seeded(): QueryClient {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: Infinity } },
  });
  client.setQueryData(qk.tasks.list({ status: "active" }), []);
  client.setQueryData(qk.plugins.list(), []);
  return client;
}

function invalidated(client: QueryClient, key: readonly unknown[]): boolean {
  return client.getQueryState(key)?.isInvalidated === true;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("QueryProvider", () => {
  it("opens the socket and invalidates from a live frame", () => {
    const captured = captureSocket();
    const client = seeded();
    render(
      <QueryProvider client={client}>
        <div />
      </QueryProvider>,
    );

    expect(daemonEvents.connect).toHaveBeenCalledOnce();
    expect(captured.event).not.toBeNull();

    captured.event?.({
      type: "task_status",
      task_id: "run-1",
      title: "",
      status: "running",
      progress_current: null,
      progress_total: null,
      result_summary: null,
      ts: "2026-08-31T12:00:00Z",
      instance_id: "7f3a1122",
      _id: 1,
    });

    expect(invalidated(client, qk.tasks.list({ status: "active" }))).toBe(true);
    expect(invalidated(client, qk.plugins.list())).toBe(false);
  });

  it("refetches everything on the possibly-missed-events signal", () => {
    const captured = captureSocket();
    const client = seeded();
    render(
      <QueryProvider client={client}>
        <div />
      </QueryProvider>,
    );

    captured.resync?.({ reason: "reconnected", offlineMs: 4200 });

    expect(invalidated(client, qk.tasks.list({ status: "active" }))).toBe(true);
    expect(invalidated(client, qk.plugins.list())).toBe(true);
  });

  it("unsubscribes and closes the socket on unmount", () => {
    const captured = captureSocket();
    const { unmount } = render(
      <QueryProvider client={seeded()}>
        <div />
      </QueryProvider>,
    );

    unmount();
    expect(captured.disposed).toBe(2);
    expect(daemonEvents.disconnect).toHaveBeenCalledOnce();
  });

  it("mounts the cache without a socket when asked", () => {
    captureSocket();
    render(
      <QueryProvider client={seeded()} connectEvents={false}>
        <div />
      </QueryProvider>,
    );
    expect(daemonEvents.connect).not.toHaveBeenCalled();
  });
});
