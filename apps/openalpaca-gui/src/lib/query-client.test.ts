/**
 * The live-event → cache bridge (API_MAP §4.2).
 *
 * Two things are asserted, because both have failed silently before: that an
 * event maps onto the right keys, and that `QueryProvider` actually subscribes
 * — a correct map behind an unmounted listener refreshes nothing.
 */

import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";

import { RUN_LOG_EVENT_TYPES } from "./api/run-events";
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

  /**
   * A follow-up queued changes what the run list will do next — the daemon
   * autostarts one when a workflow finalizes — while a cancel only shortens
   * the lane's pending queue, so it does not re-list every run.
   */
  it("refreshes the lane's queue from a follow-up frame", () => {
    expect(invalidationKeysFor(event("followup_queued"))).toEqual([
      qk.followups.all(),
      qk.tasks.all(),
    ]);
    expect(invalidationKeysFor(event("followup_cancelled"))).toEqual([
      qk.followups.all(),
    ]);
  });

  /**
   * §5.6b — the boot sweep's frame is about a run, not the conversation's
   * lifecycle, so it also stales the run list: the card a window was watching
   * has finished, and the sidebar's "N interrupted" badge has just moved.
   */
  it("adds the run list to a session frame that reports an interruption", () => {
    expect(
      invalidationKeysFor(
        event("session_changed", { status: "interrupted", task_id: "run-1" }),
      ),
    ).toEqual([qk.sessions.all(), qk.chat.all(), qk.tasks.all()]);
    expect(
      invalidationKeysFor(
        event("session_changed", { status: "archived", task_id: null }),
      ),
    ).toEqual([qk.sessions.all(), qk.chat.all()]);
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

  // T28. `["artifacts"]` is a prefix of the list, versions and diff keys, so
  // one entry refreshes the Library whichever of them is mounted. Also a
  // `run-events.ts` row (task-33 review #1), so the run's own log key comes
  // along when the frame names one.
  it("refreshes the Library when a produced artifact lands", () => {
    expect(
      invalidationKeysFor(
        event("artifact_written", { artifact_id: "a-1", task_id: "run-1" }),
      ),
    ).toEqual([qk.artifacts.all(), qk.tasks.eventLog("run-1")]);
  });

  it("refreshes the Library for a loose artifact too", () => {
    expect(
      invalidationKeysFor(
        event("artifact_written", { artifact_id: "a-2", task_id: null }),
      ),
    ).toEqual([qk.artifacts.all()]);
  });

  // Phase 4: a lane transition changes this run's timeline, not the run
  // list — a run with eight subagents fires sixteen of these over its life,
  // so invalidating `tasks.all()` here would re-list every run sixteen
  // times. Also a `run-events.ts` row (task-33 review #1), so the run's own
  // log key comes along too.
  it("refreshes the timeline and the run's own log for a lane transition", () => {
    expect(
      invalidationKeysFor(
        event("subagent_span", { task_id: "run-1", span_id: "node-1" }),
      ),
    ).toEqual([qk.tasks.timeline("run-1"), qk.tasks.eventLog("run-1")]);
  });

  // GAP-10 — these three carry a run now, so the run detail's log key is
  // refreshed alongside the daemon-wide one. That is what keeps the card live
  // now that it reads the server rather than the socket ring.
  it("refreshes the run's own log for a tool frame that names a run", () => {
    expect(
      invalidationKeysFor(
        event("tool_executed", {
          agent_id: "a1",
          tool_name: "shell_execute",
          success: true,
          duration_ms: 12,
          task_id: "run-1",
        }),
      ),
    ).toEqual([qk.events.all(), qk.tasks.eventLog("run-1")]);
  });

  it("refreshes only the daemon-wide log for a tool frame with no run", () => {
    expect(
      invalidationKeysFor(
        event("security_violation", {
          agent_id: "a1",
          tool_name: "shell_execute",
          reason: "denied",
          task_id: null,
        }),
      ),
    ).toEqual([qk.events.all()]);
  });

  it("keeps usage on an LLM frame and adds the run's log when it names one", () => {
    expect(
      invalidationKeysFor(
        event("llm_call_completed", {
          agent_id: "a1",
          model: "m",
          input_tokens: 1,
          output_tokens: 2,
          cost_usd: 0.1,
          task_id: null,
        }),
      ),
    ).toEqual([qk.usage.all()]);
    expect(
      invalidationKeysFor(
        event("llm_call_completed", {
          agent_id: "a1",
          model: "m",
          input_tokens: 1,
          output_tokens: 2,
          cost_usd: 0.1,
          task_id: "run-1",
        }),
      ),
    ).toEqual([qk.usage.all(), qk.tasks.eventLog("run-1")]);
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
      "skill_invocation_started",
      "soul_updated",
    ] as const) {
      expect(invalidationKeysFor(event(type))).toEqual([]);
    }
  });

  // task-33 review #1: this used to sit in the "purely live" group above,
  // but the confirmation is a persisted `run-events.ts` row (`tool` tag), so
  // the run-detail card needs the same refetch a tool-execution frame gets.
  it("refreshes the run's own log for a confirmation that names a run", () => {
    expect(
      invalidationKeysFor(
        event("tool_confirmation_requested", {
          request_id: "r1",
          agent_id: "a1",
          tool_name: "shell_execute",
          tool_arguments: {},
          stream_id: null,
          lane_key: null,
          task_id: "run-1",
        }),
      ),
    ).toEqual([qk.tasks.eventLog("run-1")]);
  });

  it("invalidates nothing for a confirmation with no run", () => {
    expect(
      invalidationKeysFor(
        event("tool_confirmation_requested", {
          request_id: "r1",
          agent_id: "a1",
          tool_name: "shell_execute",
          tool_arguments: {},
          stream_id: null,
          lane_key: null,
          task_id: null,
        }),
      ),
    ).toEqual([]);
  });

  // task-33 review #1 (Important finding): `subagent_span`, `artifact_written`
  // and `tool_confirmation_requested` used to skip this key even though
  // `run-events.ts` renders all three as rows on the card. This iterates
  // `RUN_LOG_EVENT_TYPES` — the same list `run-events.ts` exports — rather
  // than a second hardcoded copy, so a type added there without a matching
  // arm here fails this test instead of leaving the card stale.
  it("refreshes the run's own log for every event type run-events.ts renders", () => {
    for (const type of RUN_LOG_EVENT_TYPES) {
      const keys = invalidationKeysFor(event(type, { task_id: "run-1" }));
      expect(keys).toContainEqual(qk.tasks.eventLog("run-1"));
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
