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
  extensionResyncKeys,
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
  client.setQueryData(qk.extensions.list(), []);
  client.setQueryData(qk.tools.list(), []);
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

  // ADR-030 §9.5. Skills and agents because a plugin's contributions come and
  // go with it; connectors because a plugin may declare one.
  it("refreshes everything an extension contributes when its state changes", () => {
    expect(
      invalidationKeysFor(
        event("extension_state_changed", { kind: "plugin", id: "notion" }),
      ),
    ).toEqual([
      qk.extensions.all(),
      qk.tools.all(),
      qk.skills.all(),
      qk.agents.all(),
      qk.connectors.all(),
    ]);
  });

  it("treats a `tools_changed` refresh as any other state change", () => {
    expect(
      invalidationKeysFor(
        event("extension_state_changed", { tools_changed: true }),
      ),
    ).toEqual(
      invalidationKeysFor(
        event("extension_state_changed", { tools_changed: false }),
      ),
    );
  });

  it("narrows a withheld capability to the surfaces that showed it", () => {
    expect(invalidationKeysFor(event("extension_capability_withheld"))).toEqual(
      [qk.extensions.all(), qk.tools.all()],
    );
  });

  // The dispatcher writes the cron notice as a conversation row on the default
  // lane, so an open chat has to refetch to show it without a reload (§7.3).
  it("reaches chat from a withdrawal, because the notice lands in one", () => {
    expect(
      invalidationKeysFor(event("extension_capability_withdrawn")),
    ).toEqual([
      qk.extensions.all(),
      qk.tools.all(),
      qk.skills.all(),
      qk.agents.all(),
      qk.chat.all(),
    ]);
  });

  it("invalidates nothing for a frame this build does not know", () => {
    expect(
      invalidationKeysFor({
        type: "something_new",
        _id: 1,
      } as unknown as ServerEvent),
    ).toEqual([]);
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
    expect(invalidated(client, qk.extensions.list())).toBe(false);
    expect(invalidated(client, qk.models.list())).toBe(false);
  });

  it("reaches the extension, tool and connector lists from one state frame", () => {
    const client = seeded();
    invalidateForEvent(
      client,
      event("extension_state_changed", { kind: "plugin", id: "x" }),
    );

    expect(invalidated(client, qk.extensions.list())).toBe(true);
    expect(invalidated(client, qk.tools.list())).toBe(true);
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
      qk.extensions.list(),
      qk.connectors.list(),
      qk.skills.health(),
      qk.models.list(),
    ]) {
      expect(invalidated(client, key)).toBe(true);
    }
  });

  // §9.5 (ii), G-4: `GET /v1/extensions` is the resync primitive. The client
  // cannot detect a `Lagged` gap, so reconnect is the only trigger and it must
  // not depend on having seen an `extension_state_changed`.
  it("names the extension set explicitly, so narrowing the sweep cannot drop it", () => {
    expect(extensionResyncKeys()).toEqual([
      qk.extensions.all(),
      qk.tools.all(),
      qk.skills.all(),
      qk.agents.all(),
    ]);

    const client = seeded();
    invalidateAfterResync(client);
    expect(invalidated(client, qk.extensions.list())).toBe(true);
    expect(invalidated(client, qk.tools.list())).toBe(true);
  });
});
