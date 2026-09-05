/** The app's `QueryClient` and the WS → cache invalidation bridge. */

import { QueryClient } from "@tanstack/react-query";

import { ApiError } from "./http";
import type { ServerEvent } from "./events";
import { qk } from "./query-keys";

/** Retry transport failures and 5xx; never retry a 4xx the daemon meant. */
function shouldRetry(failureCount: number, error: unknown): boolean {
  if (error instanceof ApiError && !error.isRetryable) return false;
  return failureCount < 2;
}

export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        // The WS drives freshness; polling would double the daemon's load.
        staleTime: 30_000,
        gcTime: 5 * 60_000,
        refetchOnWindowFocus: false,
        retry: shouldRetry,
      },
      mutations: {
        retry: false,
      },
    },
  });
}

/** The app-wide client. Tests build their own with `createQueryClient()`. */
export const queryClient = createQueryClient();

/**
 * Map one live event onto the query keys it invalidates.
 *
 * Exported separately from the subscriber so it can be unit-tested and so a
 * resync burst can replay a set of keys without a socket.
 */
export function invalidationKeysFor(
  event: ServerEvent,
): readonly (readonly unknown[])[] {
  switch (event.type) {
    // `["tasks"]` is a prefix of every task key, so one entry refreshes the
    // list, the detail, the timeline and the per-run event log together.
    case "task_status":
    case "workflow_started":
    case "workflow_progress":
    case "workflow_steered":
      return [qk.tasks.all()];

    // A subagent node changes one run's timeline, not the run list, so this
    // refreshes that run only rather than re-listing on every node transition.
    case "dag_node_status":
      return [qk.tasks.detail(event.task_id), qk.tasks.timeline(event.task_id)];

    case "followup_queued":
      return [qk.followups.all(), qk.tasks.all()];

    case "chat_stream_ended":
      return [qk.chat.all(), qk.conversations.all()];

    // ADR-030 §9.5. Skills and agents because a plugin's contributions come and
    // go with it; connectors because a plugin may declare one. A frame carrying
    // `tools_changed: true` (a server-driven `tools/list_changed`, §3.7)
    // invalidates the same keys as any other.
    case "extension_state_changed":
      return [
        qk.extensions.all(),
        qk.tools.all(),
        qk.skills.all(),
        qk.agents.all(),
        qk.connectors.all(),
      ];

    // A refusal, not a transition: the surface the caller saw is stale, the
    // extension's own state is not.
    case "extension_capability_withheld":
      return [qk.extensions.all(), qk.tools.all()];

    // T1 step 3's transition event (§7.3). The dispatcher writes the cron
    // notice as a conversation row on the default lane, so an open chat has to
    // refetch to show it without a reload — the same reason
    // `chat_stream_ended` invalidates chat.
    case "extension_capability_withdrawn":
      return [
        qk.extensions.all(),
        qk.tools.all(),
        qk.skills.all(),
        qk.agents.all(),
        qk.chat.all(),
      ];

    case "connector_status":
      return [qk.connectors.all()];

    case "key_status_changed":
      return [qk.settings.all()];

    case "orchestrator_config_changed":
      return [qk.orchestrator.all(), qk.models.all()];

    case "daemon_config_changed":
      return [qk.settings.all(), qk.orchestrator.all()];

    case "skill_catalog_updated":
    case "skill_completed":
    case "skill_failed":
      return [qk.skills.all()];

    case "agent_config_changed":
      return [qk.agents.all()];

    // Not `tasks`: a run's own state arrives as `task_status`, and agents flip
    // between idle and busy far too often to re-list runs on each transition.
    case "agent_status":
      return [qk.agents.instances()];

    case "llm_call_completed":
      return [qk.usage.all()];

    case "tool_executed":
    case "security_violation":
    case "circuit_breaker_tripped":
    case "command_received":
    case "wake":
      return [qk.events.all()];

    // Purely live signals — the views that care subscribe to them directly
    // (the chat session holds the pending confirmations; the event log reads
    // the socket's own ring), so nothing cached depends on them.
    case "heartbeat":
    case "chat_stream_started":
    case "tool_confirmation_requested":
    case "skill_invocation_started":
    case "soul_updated":
      return [];

    // A frame this build does not know yet invalidates nothing rather than
    // falling off the end of the switch and returning `undefined`, which the
    // listener loop would then iterate and throw on. The daemon's event set
    // grows ahead of the GUI's.
    default:
      return [];
  }
}

/** Apply one event's invalidations. */
export function invalidateForEvent(
  client: QueryClient,
  event: ServerEvent,
): void {
  for (const queryKey of invalidationKeysFor(event)) {
    void client.invalidateQueries({ queryKey });
  }
}

/**
 * `GET /v1/extensions` is the resync primitive (ADR-030 §9.5 (ii), G-4).
 *
 * The client cannot detect a `Lagged` gap — the server warns and continues,
 * and a `resync_needed` signal is explicitly out of scope — so **reconnect is
 * the only trigger**, and it invalidates these keys unconditionally rather
 * than waiting for an `extension_state_changed` it may never see. Named
 * separately from the sweep below so narrowing that sweep can never silently
 * drop the extension set; if `resync_needed` ever ships it maps to the same
 * keys.
 */
export function extensionResyncKeys(): readonly (readonly unknown[])[] {
  return [
    qk.extensions.all(),
    qk.tools.all(),
    qk.skills.all(),
    qk.agents.all(),
  ];
}

/**
 * After a reconnect the client may have missed events with no notification
 * (the server drops frames for a lagged subscriber and never replays), so the
 * only correct move is to invalidate everything server-derived.
 */
export function invalidateAfterResync(client: QueryClient): void {
  for (const queryKey of extensionResyncKeys()) {
    void client.invalidateQueries({ queryKey });
  }
  void client.invalidateQueries();
}
