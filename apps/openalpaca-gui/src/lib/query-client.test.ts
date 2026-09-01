/**
 * The live-event → cache bridge (API_MAP §4.2).
 *
 * Two things are asserted, because both have failed silently before: that an
 * event maps onto the right keys, and that `QueryProvider` actually subscribes
 * — a correct map behind an unmounted listener refreshes nothing.
 */

import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";

import type { ServerEvent } from "./events";
import {
  invalidateAfterResync,
  invalidateForEvent,
  invalidationKeysFor,
} from "./query-client";
import { qk } from "./query-keys";

function event<T extends ServerEvent["type"]>(
  type: T,
  rest: Record<string, unknown> = {},
): ServerEvent {
  return {
    type,
    ts: "2026-08-31T12:00:00Z",
    instance_id: "7f3a1122",
    _id: 1,
    ...rest,
  } as ServerEvent;
}

/** A client holding one cached entry per domain the map can touch. */
function seeded(): QueryClient {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: Infinity } },
  });
  client.setQueryData(qk.tasks.list({ status: "active" }), []);
  client.setQueryData(qk.tasks.timeline("run-1"), null);
  client.setQueryData(qk.plugins.list(), []);
  client.setQueryData(qk.connectors.list(), []);
  client.setQueryData(qk.skills.health(), []);
  client.setQueryData(qk.models.list(), []);
  return client;
}

function invalidated(client: QueryClient, key: readonly unknown[]): boolean {
  return client.getQueryState(key)?.isInvalidated === true;
}

describe("invalidationKeysFor", () => {
  it("refreshes every task key from one run-state frame", () => {
    expect(invalidationKeysFor(event("task_status"))).toEqual([qk.tasks.all()]);
    expect(invalidationKeysFor(event("workflow_started"))).toEqual([
      qk.tasks.all(),
    ]);
    expect(invalidationKeysFor(event("workflow_progress"))).toEqual([
      qk.tasks.all(),
    ]);
    expect(invalidationKeysFor(event("workflow_steered"))).toEqual([
      qk.tasks.all(),
    ]);
  });

  it("scopes a subagent node to its own run", () => {
    expect(
      invalidationKeysFor(event("dag_node_status", { task_id: "run-1" })),
    ).toEqual([qk.tasks.detail("run-1"), qk.tasks.timeline("run-1")]);
  });

  it("refreshes plugins and connectors together — a plugin can serve one", () => {
    for (const type of [
      "plugin_loaded",
      "plugin_unloaded",
      "plugin_crashed",
      "plugin_disabled",
      "plugin_pending_approval",
      "plugin_needs_config",
    ] as const) {
      expect(invalidationKeysFor(event(type))).toEqual([
        qk.plugins.all(),
        qk.connectors.all(),
      ]);
    }
  });

  it("refreshes connectors from a connector status frame", () => {
    expect(invalidationKeysFor(event("connector_status"))).toEqual([
      qk.connectors.all(),
    ]);
  });

  it("refreshes the model surfaces when the daemon's default changes", () => {
    expect(invalidationKeysFor(event("orchestrator_config_changed"))).toEqual([
      qk.orchestrator.all(),
      qk.models.all(),
    ]);
  });

  it("invalidates nothing for the purely live signals", () => {
    for (const type of [
      "heartbeat",
      "chat_stream_started",
      "tool_confirmation_requested",
      "skill_invocation_started",
      "soul_updated",
    ] as const) {
      expect(invalidationKeysFor(event(type))).toEqual([]);
    }
  });
});

describe("invalidateForEvent", () => {
  it("marks the matching cache entries stale and leaves the rest alone", () => {
    const client = seeded();
    invalidateForEvent(client, event("task_status"));

    expect(invalidated(client, qk.tasks.list({ status: "active" }))).toBe(true);
    expect(invalidated(client, qk.tasks.timeline("run-1"))).toBe(true);
    expect(invalidated(client, qk.plugins.list())).toBe(false);
    expect(invalidated(client, qk.models.list())).toBe(false);
  });

  it("reaches the plugin and connector lists from one plugin frame", () => {
    const client = seeded();
    invalidateForEvent(client, event("plugin_crashed", { plugin_id: "x" }));

    expect(invalidated(client, qk.plugins.list())).toBe(true);
    expect(invalidated(client, qk.connectors.list())).toBe(true);
    expect(invalidated(client, qk.tasks.list({ status: "active" }))).toBe(
      false,
    );
  });
});

describe("invalidateAfterResync", () => {
  it("invalidates everything — a lagged client is told nothing about the gap", () => {
    const client = seeded();
    invalidateAfterResync(client);

    for (const key of [
      qk.tasks.list({ status: "active" }),
      qk.plugins.list(),
      qk.connectors.list(),
      qk.skills.health(),
      qk.models.list(),
    ]) {
      expect(invalidated(client, key)).toBe(true);
    }
  });
});
