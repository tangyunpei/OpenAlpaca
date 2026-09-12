/**
 * Query-key namespace.
 *
 * Every key starts with a domain segment so a WS event can invalidate a whole
 * domain (`["tasks"]`) without knowing which variants are mounted.
 */

import type { ChatHistoryQuery } from "./api/chat";
import type { ListSessionsQuery } from "./api/sessions";
import type { EventHistoryQuery } from "./api/telemetry";
import type { ListTasksQuery } from "./api/tasks";
import type { LlmUsageQuery } from "./api/usage";
import type { TelemetryQuery } from "./api/orchestrator";

export const qk = {
  health: () => ["health"] as const,

  /**
   * `GET /v1/status`. The window's project is part of the key because the
   * route answers a question *about the request* — a different
   * `x-workspace-path` is a different answer, not a stale one.
   */
  status: (workspacePath: string | null) => ["status", workspacePath] as const,

  chat: {
    all: () => ["chat"] as const,
    history: (query: ChatHistoryQuery) => ["chat", "history", query] as const,
    feedback: (messageId: number) => ["chat", "feedback", messageId] as const,
  },

  sessions: {
    all: () => ["sessions"] as const,
    list: (query: ListSessionsQuery) => ["sessions", "list", query] as const,
    messages: (
      id: string,
      query: { limit?: number; offset?: number; before_id?: number },
    ) => ["sessions", "messages", id, query] as const,
  },

  tasks: {
    all: () => ["tasks"] as const,
    list: (query: ListTasksQuery) => ["tasks", "list", query] as const,
    detail: (id: string) => ["tasks", "detail", id] as const,
    timeline: (id: string) => ["tasks", "timeline", id] as const,
    eventLog: (id: string) => ["tasks", "event-log", id] as const,
  },

  files: {
    all: () => ["files"] as const,
    metadata: (id: string) => ["files", "metadata", id] as const,
    content: (id: string) => ["files", "content", id] as const,
  },

  artifacts: {
    all: () => ["artifacts"] as const,
    list: (query: Record<string, unknown>) =>
      ["artifacts", "list", query] as const,
    detail: (id: string) => ["artifacts", "detail", id] as const,
    content: (id: string) => ["artifacts", "content", id] as const,
    versions: (id: string) => ["artifacts", "versions", id] as const,
    diff: (id: string, from: number, to: number) =>
      ["artifacts", "diff", id, from, to] as const,
  },

  settings: {
    all: () => ["settings"] as const,
    llm: () => ["settings", "llm"] as const,
    keyStatus: () => ["settings", "key-status"] as const,
    providerUsage: () => ["settings", "provider-usage"] as const,
    credentials: () => ["settings", "credentials"] as const,
    cliBackends: () => ["settings", "cli-backends"] as const,
  },

  models: {
    all: () => ["models"] as const,
    list: () => ["models", "list"] as const,
  },

  usage: {
    all: () => ["usage"] as const,
    calls: (query: LlmUsageQuery) => ["usage", "calls", query] as const,
    daily: (query: { agentId?: string; date?: string; limit?: number }) =>
      ["usage", "daily", query] as const,
    /** `GET /v1/usage/summary?window=today` — the daemon picks the day. */
    summary: () => ["usage", "summary"] as const,
  },

  orchestrator: {
    all: () => ["orchestrator"] as const,
    config: () => ["orchestrator", "config"] as const,
    latency: (query: TelemetryQuery) =>
      ["orchestrator", "latency", query] as const,
    latencyAggregate: (query: { from?: string; to?: string }) =>
      ["orchestrator", "latency-aggregate", query] as const,
    decisions: (query: TelemetryQuery) =>
      ["orchestrator", "decisions", query] as const,
  },

  /** MCP servers and plugins in one list (ADR-030 §9.2). */
  extensions: {
    all: () => ["extensions"] as const,
    list: () => ["extensions", "list"] as const,
  },

  tools: {
    all: () => ["tools"] as const,
    list: () => ["tools", "list"] as const,
  },

  connectors: {
    all: () => ["connectors"] as const,
    list: () => ["connectors", "list"] as const,
    settings: (id: string) => ["connectors", "settings", id] as const,
  },

  skills: {
    all: () => ["skills"] as const,
    health: () => ["skills", "health"] as const,
    catalog: () => ["skills", "catalog"] as const,
  },

  agents: {
    all: () => ["agents"] as const,
    templates: () => ["agents", "templates"] as const,
    template: (id: string) => ["agents", "template", id] as const,
    instances: () => ["agents", "instances"] as const,
    detail: (id: string) => ["agents", "detail", id] as const,
    config: (id: string) => ["agents", "config", id] as const,
  },

  events: {
    all: () => ["events"] as const,
    history: (query: EventHistoryQuery) =>
      ["events", "history", query] as const,
  },

  followups: {
    all: () => ["followups"] as const,
    list: (laneKey: string) => ["followups", "list", laneKey] as const,
  },

  daemon: {
    all: () => ["daemon"] as const,
    statusDetail: () => ["daemon", "status-detail"] as const,
  },

  /**
   * `GET /v1/workspaces?path=`. The path is the key: the route answers about
   * one root, and another root is a different answer, not a stale one.
   */
  workspaces: {
    all: () => ["workspaces"] as const,
    detail: (path: string) => ["workspaces", "detail", path] as const,
  },
} as const;
