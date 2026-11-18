/**
 * The API contract, this side (API-CONTRACT): every sample in `./contract/`
 * — written by the daemon's route tests (`apps/openalpacad/src/contract/`) —
 * parses against the TypeScript type the GUI reads it as, and every type
 * listed here has its sample. A response the daemon changes without the
 * type following fails here, naming the sample and the type.
 *
 * The descriptors are tied to the types by `tsc` (`./contract-shape.ts`),
 * so they are not a second copy of the contract: a type that moves without
 * its descriptor fails `bun run check`.
 */

import { describe, expect, it } from "vitest";

import type { ServerEvent } from "../events";
import {
  createAgentTemplate,
  deleteAgentTemplate,
  updateAgentTemplate,
} from "./agents";
import type {
  ArtifactDiff,
  ArtifactPinState,
  ArtifactVersion,
  WireArtifact,
} from "./artifacts";
import { check, mapOf, shape, type Inner, type Node } from "./contract-shape";
import type { FollowupCancelled, FollowupRecord } from "./followups";
import type {
  MemoryForgotten,
  MemoryListItem,
  MemoryListPage,
  MemoryNote,
  MemoryRevision,
  MemorySearchPage,
  MemoryStoreResult,
  MemorySuggestion,
  MemoryUpdateResult,
  MemoryWithdrawn,
} from "./memories";
import type { ShutdownAccepted } from "./status";
import type { RerunResult, SteerResult, TaskTimeline } from "./tasks";
import type { EventHistoryPage } from "./telemetry";
import type {
  Project,
  ProjectDataDeletion,
  ProjectRepoint,
  Workspace,
} from "./workspaces";
import type {
  AgentActionResponse,
  AgentConfigResponse,
  AgentDetailResponse,
  AgentInstance,
  AgentTemplate,
  ChatDeleteResponse,
  ChatHistoryResponse,
  ChatSendResponse,
  CliBackendStatus,
  Connector,
  CreateTaskResponse,
  DaemonStatus,
  DiscoveredCredentialInfo,
  DispatchDecisionRecord,
  ExtensionRow,
  FeedbackResponse,
  FileAsset,
  FileOpenResponse,
  FileUploadResponse,
  HealthResponse,
  InstallResponse,
  KeyStatusMap,
  KeyValidationResult,
  LatencyAggregate,
  LlmCallLog,
  LlmSettingsResponse,
  LlmUsageDaily,
  ModelEntry,
  OrchestratorConfigResponse,
  OrchestratorLatencyRecord,
  ParsedOutcome,
  PendingConfirmationsResponse,
  ProviderEnabledResponse,
  ProviderUsageSummary,
  Session,
  SessionMessagesResponse,
  SessionsResponse,
  SkillCatalogEntry,
  SkillDiagnostic,
  SkillHealthMetrics,
  Task,
  TaskActionResponse,
  TaskDetailResponse,
  ToolCatalogEntry,
  UninstallResponse,
  UsageSummary,
  ValidateResponse,
  WorkingState,
} from "./types";

// ── The samples ─────────────────────────────────────────────────────────────

function byName(files: Record<string, unknown>): Map<string, unknown> {
  return new Map(
    Object.entries(files).map(([path, value]) => [
      path.replace(/^.*\//, "").replace(/\.json$/, ""),
      value,
    ]),
  );
}

const responseSamples = byName(
  import.meta.glob("./contract/responses/*.json", {
    eager: true,
    import: "default",
  }),
);
const eventSamples = byName(
  import.meta.glob("./contract/events/*.json", {
    eager: true,
    import: "default",
  }),
);

// ── The types they are read as ──────────────────────────────────────────────

interface Entry {
  /** The type the GUI reads the body as, for the failure's sentence. */
  readonly type: string;
  readonly node: Node<unknown>;
}

/** A body read as `T` (an array's element, where `T` is an array). */
function body<T>(type: string, node: Node<Inner<T>>): Entry {
  return { type, node: node as Node<unknown> };
}

const artifactShape = shape<WireArtifact>(
  {
    id: "required",
    name: "required",
    kind: "required",
    mime_type: "required",
    size_bytes: "required",
    task_id: "required",
    task_title: "required",
    agent_id: "required",
    agent_template_id: "required",
    version: "required",
    version_count: "required",
    summary: "required",
    metadata: "required",
    created_at: "required",
    updated_at: "required",
    origin: "required",
    pinned: "required",
    missing: "required",
    path: "required",
    project_root: "required",
    rel_path: "required",
    in_project: "optional",
    openable: "optional",
  },
  { metadata: "opaque" },
);

/** A conversation's working state (CQ-1), on every session row. */
const workingStateShape = shape<WorkingState>(
  {
    v: "required",
    topic: "required",
    goal: "required",
    done: "required",
    asked: "required",
    decisions: "required",
    open: "required",
    next: "required",
    refs: "required",
  },
  {},
);

/** A model the router can reach (`GET /v1/models`, and its refresh). */
const modelEntryShape = shape<ModelEntry>(
  {
    context_window: "required",
    id: "required",
    input_price_per_million: "required",
    output_price_per_million: "required",
    provider: "required",
    supports_tools: "required",
  },
  {},
);

/** A finished run's parsed outcome, on a run row and beside it. */
const parsedOutcomeShape = shape<ParsedOutcome>(
  {
    outcome_summary: "required",
    outcome_kind: "required",
    artifact_count: "required",
    artifacts: "required",
    no_artifact_reason: "optional",
  },
  {},
);

/** Each response sample, by name, and the type the wrapper reads it as. */
const RESPONSES: Record<string, Entry> = {
  "get-health": body<HealthResponse>(
    "HealthResponse",
    shape<HealthResponse>(
      {
        status: "required",
        version: "required",
        pid: "required",
        instance_id: "required",
        protocol: "optional",
      },
      {},
    ),
  ),
  "get-artifacts": body<{ artifacts: WireArtifact[]; total: number }>(
    "{ artifacts: WireArtifact[]; total: number }",
    shape<{ artifacts: WireArtifact[]; total: number }>(
      { artifacts: "required", total: "required" },
      { artifacts: artifactShape },
    ),
  ),
  "get-artifacts-id": body<WireArtifact>("WireArtifact", artifactShape),
  "get-artifacts-id-versions": body<{ versions: ArtifactVersion[] }>(
    "{ versions: ArtifactVersion[] }",
    shape<{ versions: ArtifactVersion[] }>(
      { versions: "required" },
      {
        versions: shape<ArtifactVersion>(
          {
            version: "required",
            note: "required",
            author_agent_id: "required",
            created_at: "required",
            size_bytes: "required",
            added_lines: "required",
            removed_lines: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-artifacts-id-diff": body<ArtifactDiff>(
    "ArtifactDiff",
    shape<ArtifactDiff>(
      {
        from: "required",
        to: "required",
        added_lines: "required",
        removed_lines: "required",
        format: "required",
        patch: "required",
      },
      {},
    ),
  ),
  "put-artifacts-id-pin": body<ArtifactPinState>(
    "ArtifactPinState",
    shape<ArtifactPinState>({ id: "required", pinned: "required" }, {}),
  ),
  "delete-agent-templates-id": body<
    Awaited<ReturnType<typeof deleteAgentTemplate>>
  >(
    "deleteAgentTemplate's answer",
    shape<Inner<Awaited<ReturnType<typeof deleteAgentTemplate>>>>(
      { status: "required", template_id: "required" },
      {},
    ),
  ),
  "delete-chat-history": body<ChatDeleteResponse>(
    "ChatDeleteResponse",
    shape<ChatDeleteResponse>({ deleted: "required" }, {}),
  ),
  "delete-extensions-kind-id": body<UninstallResponse>(
    "UninstallResponse",
    shape<UninstallResponse>(
      {
        data_trashed: "required",
        kept_data: "required",
        removed: "required",
        trashed: "required",
      },
      {},
    ),
  ),
  "delete-lanes-key-followups-id": body<FollowupCancelled>(
    "FollowupCancelled",
    shape<FollowupCancelled>({ id: "required", status: "required" }, {}),
  ),
  "delete-memories-id": body<MemoryForgotten>(
    "MemoryForgotten",
    shape<MemoryForgotten>(
      { id: "required", retained: "required", status: "required" },
      {
        retained: shape<Inner<MemoryForgotten["retained"]>>(
          {
            note: "required",
            session_logs: "required",
            transcript: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-agent-instances": body<AgentInstance[]>(
    "AgentInstance[]",
    shape<AgentInstance>(
      {
        current_task: "required",
        id: "required",
        name: "required",
        status: "required",
        template_id: "required",
      },
      {},
    ),
  ),
  "get-agent-templates": body<AgentTemplate[]>(
    "AgentTemplate[]",
    shape<AgentTemplate>(
      {
        icon: "optional",
        temperature: "optional",
        max_tool_calls: "optional",
        timeout_seconds: "optional",
        max_cost_per_task: "optional",
        last_run_at: "optional",
        body: "required",
        capabilities: "required",
        denied_capabilities: "required",
        description: "required",
        fallback_models: "required",
        id: "required",
        model: "optional",
        name: "required",
        not_used: "optional",
        overrides_bundled: "optional",
        path: "optional",
        persona: "required",
        plugin: "optional",
        require_confirmation_for: "required",
        run_count: "required",
        singleton: "required",
        source: "optional",
        state: "optional",
        verbosity: "required",
        warnings: "optional",
        window: "required",
      },
      {
        not_used: shape<Inner<AgentTemplate["not_used"]>>(
          {
            edit: "required",
            path: "required",
            plugin: "required",
            reason: "required",
            source: "required",
            version: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-agent-templates-id": body<AgentTemplate>(
    "AgentTemplate",
    shape<AgentTemplate>(
      {
        icon: "optional",
        temperature: "optional",
        max_tool_calls: "optional",
        timeout_seconds: "optional",
        max_cost_per_task: "optional",
        last_run_at: "optional",
        body: "required",
        capabilities: "required",
        denied_capabilities: "required",
        description: "required",
        fallback_models: "required",
        id: "required",
        model: "optional",
        name: "required",
        not_used: "optional",
        overrides_bundled: "optional",
        path: "optional",
        persona: "required",
        plugin: "optional",
        require_confirmation_for: "required",
        run_count: "required",
        singleton: "required",
        source: "optional",
        state: "optional",
        verbosity: "required",
        warnings: "optional",
        window: "required",
      },
      {
        not_used: shape<Inner<AgentTemplate["not_used"]>>(
          {
            edit: "required",
            path: "required",
            plugin: "required",
            reason: "required",
            source: "required",
            version: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-agents-id": body<AgentDetailResponse>(
    "AgentDetailResponse",
    shape<AgentDetailResponse>(
      { agent: "required", metrics: "required" },
      {
        agent: shape<Inner<AgentDetailResponse["agent"]>>(
          {
            constraints_json: "required",
            created_at: "required",
            current_task_id: "required",
            description: "required",
            icon: "required",
            id: "required",
            llm_config_json: "required",
            name: "required",
            persona: "required",
            preset_json: "required",
            skills_json: "required",
            status: "required",
            template_id: "optional",
            updated_at: "required",
          },
          {},
        ),
        metrics: shape<Inner<AgentDetailResponse["metrics"]>>(
          {
            agent_id: "required",
            average_runtime_seconds: "required",
            success_rate: "required",
            tasks_completed: "required",
            tasks_failed: "required",
            total_runtime_seconds: "required",
            updated_at: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-agents-id-config": body<AgentConfigResponse>(
    "AgentConfigResponse",
    shape<AgentConfigResponse>(
      { config: "required", config_version: "required" },
      {
        config: shape<Inner<AgentConfigResponse["config"]>>(
          {
            capabilities: "required",
            agent: "required",
            constraints: "optional",
            llm: "optional",
            preset: "required",
          },
          {
            capabilities: shape<
              Inner<Inner<AgentConfigResponse["config"]>["capabilities"]>
            >(
              {
                assigned: "required",
                denied: "optional",
              },
              {},
            ),
            agent: shape<Inner<Inner<AgentConfigResponse["config"]>["agent"]>>(
              {
                description: "required",
                icon: "optional",
                id: "required",
                name: "required",
              },
              {},
            ),
            constraints: shape<
              Inner<Inner<AgentConfigResponse["config"]>["constraints"]>
            >(
              {
                max_rounds: "optional",
                allowed_capabilities: "optional",
                denied_capabilities: "optional",
                max_cost_per_task: "optional",
                max_tool_calls: "optional",
                require_confirmation_for: "optional",
                timeout_seconds: "optional",
              },
              {},
            ),
            llm: shape<Inner<Inner<AgentConfigResponse["config"]>["llm"]>>(
              {
                fallback_models: "optional",
                model: "optional",
              },
              {},
            ),
            preset: shape<
              Inner<Inner<AgentConfigResponse["config"]>["preset"]>
            >(
              {
                temperature: "optional",
                persona: "required",
                verbosity: "optional",
              },
              {},
            ),
          },
        ),
      },
    ),
  ),
  "get-chat-confirmations": body<PendingConfirmationsResponse>(
    "PendingConfirmationsResponse",
    shape<PendingConfirmationsResponse>(
      { confirmations: "required" },
      {
        confirmations: shape<
          Inner<PendingConfirmationsResponse["confirmations"]>
        >(
          {
            agent_id: "required",
            lane_key: "required",
            per_call: "optional",
            preview: "optional",
            raised_at: "required",
            request_id: "required",
            task_id: "required",
            tool_arguments: "required",
            tool_name: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-chat-history": body<ChatHistoryResponse>(
    "ChatHistoryResponse",
    shape<ChatHistoryResponse>(
      {
        lane_key: "required",
        messages: "required",
        session_id: "required",
        total: "required",
      },
      {
        messages: shape<Inner<ChatHistoryResponse["messages"]>>(
          {
            // No daemon field: no route sends it and no view reads it (a
            // finding, CONTRACT-SAMPLES-2).
            confirmation: "may_omit",
            artifacts: "optional",
            content: "required",
            content_json: "optional",
            created_at: "required",
            display_text: "optional",
            duration_ms: "optional",
            id: "required",
            lane_key: "required",
            model: "optional",
            role: "required",
            source: "optional",
            task_id: "optional",
            tokens_in: "optional",
            tokens_out: "optional",
          },
          {
            confirmation: shape<
              Inner<Inner<ChatHistoryResponse["messages"]>["confirmation"]>
            >(
              {
                request_id: "required",
                tool_name: "required",
                tool_arguments: "required",
                status: "required",
              },
              {},
            ),
            artifacts: shape<
              Inner<Inner<ChatHistoryResponse["messages"]>["artifacts"]>
            >({ id: "required", kind: "required", name: "required" }, {}),
          },
        ),
      },
    ),
  ),
  "get-chat-messages-id-feedback": body<FeedbackResponse>(
    "FeedbackResponse",
    shape<FeedbackResponse>(
      { comment: "required", feedback: "required", message_id: "required" },
      {},
    ),
  ),
  "get-connectors": body<Connector[]>(
    "Connector[]",
    shape<Connector>(
      {
        configured: "required",
        id: "required",
        messages_7d: "required",
        name: "required",
        registered: "required",
        source: "required",
        status: "required",
      },
      {},
    ),
  ),
  "get-connectors-id-settings": body<Record<string, string | null>>(
    "Record<string, string | null>",
    mapOf<Record<string, string | null>>("opaque"),
  ),
  "get-events-history": body<EventHistoryPage>(
    "EventHistoryPage",
    shape<EventHistoryPage>(
      { events: "required", next_before: "required" },
      {
        events: shape<Inner<EventHistoryPage["events"]>>(
          {
            agent_id: "required",
            detail: "optional",
            event_type: "required",
            id: "required",
            result: "optional",
            task_id: "required",
            timestamp: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-extensions": body<ExtensionRow[]>(
    "ExtensionRow[]",
    shape<ExtensionRow>(
      {
        // Per-call: only the verb that produced a warning sends it (row_json).
        warnings: "may_omit",
        actionable: "required",
        added_capabilities: "required",
        agents: "required",
        connect_ms: "optional",
        connector: "required",
        consent: "required",
        declared: "required",
        detail: "required",
        enabled: "required",
        hint: "required",
        id: "required",
        kind: "required",
        list_ms: "optional",
        missing_config_keys: "required",
        protocol_version: "optional",
        provider: "required",
        reason: "required",
        since: "required",
        skills: "required",
        skipped_tools: "required",
        state: "required",
        tools: "required",
        tools_changed_at: "required",
        transport: "required",
        version: "required",
        withdrawn_by_server: "required",
      },
      {
        declared: shape<Inner<ExtensionRow["declared"]>>(
          {
            capabilities: "required",
            types: "required",
            virtual_capabilities: "required",
            warnings: "optional",
          },
          {
            types: "opaque",
          },
        ),
      },
    ),
  ),
  "get-files-id": body<FileAsset>(
    "FileAsset",
    shape<FileAsset>(
      {
        created_at: "required",
        extract_error: "required",
        extracted_text: "required",
        filename: "required",
        id: "required",
        metadata_json: "required",
        mime_type: "required",
        owner_id: "required",
        sha256: "required",
        size_bytes: "required",
        status: "required",
        storage_path: "required",
        updated_at: "required",
      },
      {},
    ),
  ),
  "get-lanes-key-followups": body<FollowupRecord[]>(
    "FollowupRecord[]",
    shape<FollowupRecord>(
      {
        content: "required",
        created_at: "required",
        id: "required",
        kind: "required",
        lane_key: "required",
        source_task_id: "required",
        status: "required",
        updated_at: "required",
      },
      {},
    ),
  ),
  "get-llm-usage": body<LlmCallLog[]>(
    "LlmCallLog[]",
    shape<LlmCallLog>(
      {
        agent_id: "required",
        cost_usd: "required",
        error_message: "required",
        id: "required",
        input_tokens: "required",
        key_id: "required",
        latency_ms: "required",
        model: "required",
        output_tokens: "required",
        provider: "required",
        status: "required",
        task_id: "required",
        timestamp: "required",
      },
      {},
    ),
  ),
  "get-llm-usage-daily": body<LlmUsageDaily[]>(
    "LlmUsageDaily[]",
    shape<LlmUsageDaily>(
      {
        agent_id: "required",
        date: "required",
        model: "required",
        total_cost_usd: "required",
        total_input_tokens: "required",
        total_output_tokens: "required",
        total_requests: "required",
      },
      {},
    ),
  ),
  "get-memories": body<MemoryListPage>(
    "MemoryListPage",
    shape<MemoryListPage>(
      { items: "required", next_cursor: "required" },
      {
        items: shape<Inner<MemoryListPage["items"]>>(
          {
            created_at: "required",
            effective_status: "required",
            id: "required",
            kind: "required",
            origin: "required",
            pinned: "required",
            revision: "required",
            scope: "required",
            status: "required",
            supersedes: "required",
            title: "required",
            updated_at: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-memories-id": body<MemoryNote>(
    "MemoryNote",
    shape<MemoryNote>(
      {
        body: "required",
        conflict: "required",
        effective_status: "required",
        freshness: "required",
        frontmatter: "required",
        id: "required",
        kind: "required",
        origin: "required",
        path: "required",
        revision: "required",
        scope: "required",
        sources: "required",
        sources_status: "required",
        status: "required",
        superseded_by: "required",
        supersedes: "required",
        title: "required",
        truncated: "required",
      },
      {
        sources: shape<Inner<MemoryNote["sources"]>>(
          {
            type: "required",
            id: "required",
            observed_at: "required",
          },
          {},
        ),
        frontmatter: "opaque",
      },
    ),
  ),
  "get-memories-id-history": body<{ revisions: MemoryRevision[] }>(
    "{ revisions: MemoryRevision[] }",
    shape<{ revisions: MemoryRevision[] }>(
      { revisions: "required" },
      {
        revisions: shape<Inner<{ revisions: MemoryRevision[] }["revisions"]>>(
          { revision: "required", written_at: "required" },
          {},
        ),
      },
    ),
  ),
  "get-memories-recently-learned": body<{ items: MemoryListItem[] }>(
    "{ items: MemoryListItem[] }",
    shape<{ items: MemoryListItem[] }>(
      { items: "required" },
      {
        items: shape<Inner<{ items: MemoryListItem[] }["items"]>>(
          {
            created_at: "required",
            effective_status: "required",
            id: "required",
            kind: "required",
            origin: "required",
            pinned: "required",
            revision: "required",
            scope: "required",
            status: "required",
            supersedes: "required",
            title: "required",
            updated_at: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-memories-search": body<MemorySearchPage>(
    "MemorySearchPage",
    shape<MemorySearchPage>(
      {
        count: "required",
        index_state: "required",
        partial: "required",
        results: "required",
      },
      {
        results: shape<Inner<MemorySearchPage["results"]>>(
          {
            conflict: "required",
            effective_status: "required",
            excerpt: "required",
            freshness: "required",
            heading: "required",
            id: "required",
            kind: "required",
            line_range: "required",
            match: "required",
            origin: "required",
            path: "required",
            pinned: "required",
            revision: "required",
            same_key_loser: "required",
            scope: "required",
            sources: "required",
            sources_status: "required",
            status: "required",
            superseded_by: "required",
            title: "required",
          },
          {
            sources: shape<
              Inner<Inner<MemorySearchPage["results"]>["sources"]>
            >(
              {
                type: "required",
                id: "required",
                observed_at: "required",
              },
              {},
            ),
          },
        ),
      },
    ),
  ),
  "get-memories-suggestions": body<{ items: MemorySuggestion[] }>(
    "{ items: MemorySuggestion[] }",
    shape<{ items: MemorySuggestion[] }>(
      { items: "required" },
      {
        items: shape<Inner<{ items: MemorySuggestion[] }["items"]>>(
          {
            changes: "required",
            created_at: "required",
            id: "required",
            knowledge: "required",
            knowledge_left_out: "required",
            observed_at: "required",
            older_count: "required",
            origin: "required",
            reason: "required",
            revision: "required",
            scope: "required",
            sources: "required",
            target: "required",
            title: "required",
          },
          {
            changes: shape<
              Inner<Inner<{ items: MemorySuggestion[] }["items"]>["changes"]>
            >(
              {
                field: "required",
                from: "required",
                to: "required",
              },
              {},
            ),
            sources: shape<
              Inner<Inner<{ items: MemorySuggestion[] }["items"]>["sources"]>
            >(
              {
                type: "required",
                id: "required",
                observed_at: "required",
              },
              {},
            ),
            target: shape<
              Inner<Inner<{ items: MemorySuggestion[] }["items"]>["target"]>
            >(
              {
                excerpt: "required",
                excerpt_left_out: "required",
                id: "required",
                origin: "required",
                revision: "required",
                scope: "required",
                title: "required",
              },
              {},
            ),
          },
        ),
      },
    ),
  ),
  "get-models": body<ModelEntry[]>("ModelEntry[]", modelEntryShape),
  "get-orchestrator-config": body<OrchestratorConfigResponse>(
    "OrchestratorConfigResponse",
    shape<OrchestratorConfigResponse>(
      {
        active_agents: "required",
        active_tasks: "required",
        daily_cost_usd: "required",
        fallback_models: "required",
        model: "required",
      },
      {},
    ),
  ),
  "get-orchestrator-decisions": body<{ records: DispatchDecisionRecord[] }>(
    "{ records: DispatchDecisionRecord[] }",
    shape<{ records: DispatchDecisionRecord[] }>(
      { records: "required" },
      {
        records: shape<Inner<{ records: DispatchDecisionRecord[] }["records"]>>(
          {
            agent_count: "required",
            dag_node_count: "required",
            error_message: "required",
            id: "required",
            mode: "required",
            predictability_score: "required",
            reason: "required",
            request_id: "required",
            task_id: "required",
            timestamp: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-orchestrator-latency": body<{ records: OrchestratorLatencyRecord[] }>(
    "{ records: OrchestratorLatencyRecord[] }",
    shape<{ records: OrchestratorLatencyRecord[] }>(
      { records: "required" },
      {
        records: shape<
          Inner<{ records: OrchestratorLatencyRecord[] }["records"]>
        >(
          {
            ack_ms: "required",
            auto_promotion_reason: "required",
            fallback_reason: "required",
            id: "required",
            mode: "required",
            request_id: "required",
            timestamp: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-orchestrator-latency-aggregate": body<{
    aggregates: LatencyAggregate[];
  }>(
    "{ aggregates: LatencyAggregate[] }",
    shape<{ aggregates: LatencyAggregate[] }>(
      { aggregates: "required" },
      {
        aggregates: shape<
          Inner<{ aggregates: LatencyAggregate[] }["aggregates"]>
        >(
          {
            auto_promotion_count: "required",
            count: "required",
            fallback_count: "required",
            mean_ack_ms: "required",
            mode: "required",
            p50_total_ms: "required",
            p95_total_ms: "required",
            p99_total_ms: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-persona-user": body<{ content: string }>(
    "{ content: string }",
    shape<{ content: string }>({ content: "required" }, {}),
  ),
  "get-sessions": body<SessionsResponse>(
    "SessionsResponse",
    shape<SessionsResponse>(
      { sessions: "required", total: "required" },
      {
        sessions: shape<Inner<SessionsResponse["sessions"]>>(
          {
            active_task_count: "required",
            created_at: "required",
            ended_at: "required",
            id: "required",
            interrupted_task_count: "required",
            lane_key: "required",
            last_message_at: "required",
            message_count: "required",
            source: "required",
            status: "required",
            title: "required",
            updated_at: "required",
            working_state: "required",
            working_state_updated_at: "required",
            workspace_id: "required",
          },
          {
            working_state: workingStateShape,
          },
        ),
      },
    ),
  ),
  "get-sessions-id-messages": body<SessionMessagesResponse>(
    "SessionMessagesResponse",
    shape<SessionMessagesResponse>(
      { messages: "required", total: "required" },
      {
        messages: shape<Inner<SessionMessagesResponse["messages"]>>(
          {
            // No daemon field: no route sends it and no view reads it (a
            // finding, CONTRACT-SAMPLES-2).
            confirmation: "may_omit",
            artifacts: "optional",
            content: "required",
            content_json: "optional",
            created_at: "required",
            display_text: "optional",
            duration_ms: "optional",
            id: "required",
            lane_key: "required",
            model: "optional",
            role: "required",
            source: "optional",
            task_id: "optional",
            tokens_in: "optional",
            tokens_out: "optional",
          },
          {
            artifacts: shape<
              Inner<Inner<SessionMessagesResponse["messages"]>["artifacts"]>
            >(
              {
                id: "required",
                name: "required",
                kind: "required",
              },
              {},
            ),
            confirmation: shape<
              Inner<Inner<SessionMessagesResponse["messages"]>["confirmation"]>
            >(
              {
                request_id: "required",
                tool_name: "required",
                tool_arguments: "required",
                status: "required",
              },
              {},
            ),
          },
        ),
      },
    ),
  ),
  "get-settings-llm": body<LlmSettingsResponse>(
    "LlmSettingsResponse",
    shape<LlmSettingsResponse>(
      { orchestrator: "required", providers: "required" },
      {
        orchestrator: shape<Inner<LlmSettingsResponse["orchestrator"]>>(
          { fallback_models: "required", model: "required" },
          {},
        ),
        providers: mapOf<Inner<LlmSettingsResponse["providers"]>>(
          shape<
            Inner<
              Inner<LlmSettingsResponse["providers"]>[keyof Inner<
                LlmSettingsResponse["providers"]
              >]
            >
          >(
            {
              enabled: "required",
              key_selection_strategy: "required",
              keys: "required",
              requires_key: "required",
            },
            {
              keys: shape<
                Inner<
                  Inner<
                    Inner<LlmSettingsResponse["providers"]>[keyof Inner<
                      LlmSettingsResponse["providers"]
                    >]
                  >["keys"]
                >
              >(
                {
                  // skip_serializing_if; settings_service never sets it (a finding).
                  managed: "may_omit",
                  // skip_serializing_if; settings_service never sets it (a finding).
                  credential_status: "may_omit",
                  // skip_serializing_if; settings_service never sets it (a finding).
                  credential_expires_at: "may_omit",
                  // skip_serializing_if; settings_service never sets it (a finding).
                  external_usage: "may_omit",
                  id: "required",
                  masked_secret: "required",
                  monthly_usage_usd: "required",
                  notes: "required",
                  priority: "required",
                  source: "required",
                  status: "required",
                  tier: "required",
                },
                {
                  external_usage: shape<
                    Inner<
                      Inner<
                        Inner<
                          Inner<LlmSettingsResponse["providers"]>[keyof Inner<
                            LlmSettingsResponse["providers"]
                          >]
                        >["keys"]
                      >["external_usage"]
                    >
                  >(
                    {
                      period: "required",
                      cost_usd: "required",
                      token_count: "required",
                      rate_limit_remaining: "required",
                      fetched_at: "required",
                      approximate: "required",
                    },
                    {},
                  ),
                },
              ),
            },
          ),
        ),
      },
    ),
  ),
  "get-settings-llm-cli-backends": body<CliBackendStatus[]>(
    "CliBackendStatus[]",
    shape<CliBackendStatus>(
      {
        available: "required",
        enabled: "required",
        name: "required",
        path: "required",
      },
      {},
    ),
  ),
  "get-settings-llm-credentials": body<DiscoveredCredentialInfo[]>(
    "DiscoveredCredentialInfo[]",
    shape<DiscoveredCredentialInfo>(
      {
        auto_refresh: "required",
        expires_at: "required",
        provider: "required",
        source: "required",
        status: "required",
      },
      {},
    ),
  ),
  "get-settings-llm-providers-usage": body<ProviderUsageSummary[]>(
    "ProviderUsageSummary[]",
    shape<ProviderUsageSummary>(
      {
        external_usage: "required",
        health: "required",
        provider: "required",
        total_cost_usd: "required",
        total_requests: "required",
        total_tokens: "required",
      },
      {
        external_usage: shape<Inner<ProviderUsageSummary["external_usage"]>>(
          {
            approximate: "required",
            cost_usd: "required",
            fetched_at: "required",
            period: "required",
            rate_limit_remaining: "required",
            token_count: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-settings-llm-status": body<KeyStatusMap>(
    "KeyStatusMap",
    mapOf<KeyStatusMap>(
      shape<Inner<KeyStatusMap[keyof KeyStatusMap]>>(
        {
          consecutive_rate_limits: "required",
          health: "required",
          id: "required",
          is_available: "required",
        },
        {},
      ),
    ),
  ),
  "get-skills": body<SkillCatalogEntry[]>(
    "SkillCatalogEntry[]",
    shape<SkillCatalogEntry>(
      {
        author: "required",
        description: "required",
        id: "required",
        invocations_today: "required",
        name: "required",
        not_used: "optional",
        origin: "required",
        overrides_bundled: "optional",
        requires_capabilities: "required",
        schedule: "required",
        source: "required",
        triggers: "required",
        version: "required",
      },
      {
        not_used: shape<Inner<SkillCatalogEntry["not_used"]>>(
          {
            edit: "required",
            path: "required",
            plugin: "required",
            reason: "required",
            source: "required",
            version: "required",
          },
          {},
        ),
        origin: shape<Inner<SkillCatalogEntry["origin"]>>(
          {
            enabled: "required",
            id: "required",
            kind: "required",
            state: "required",
          },
          {},
        ),
        triggers: shape<Inner<SkillCatalogEntry["triggers"]>>(
          { keywords: "required", slash: "required" },
          {},
        ),
      },
    ),
  ),
  "get-skills-diagnostics": body<SkillDiagnostic[]>(
    "SkillDiagnostic[]",
    shape<SkillDiagnostic>(
      {
        code: "required",
        message: "required",
        path: "required",
        severity: "required",
        skill_id: "required",
      },
      {},
    ),
  ),
  "get-skills-health": body<SkillHealthMetrics[]>(
    "SkillHealthMetrics[]",
    shape<SkillHealthMetrics>(
      {
        avg_cost_usd: "required",
        avg_duration_ms: "required",
        avg_rounds: "required",
        clean_success_rate: "required",
        clean_success_rate_7d: "required",
        degraded_rate: "required",
        feedback_count: "required",
        feedback_coverage: "required",
        last_invoked_at: "required",
        repair_effectiveness: "required",
        repair_rate: "required",
        skill_id: "required",
        total_invocations: "required",
        user_satisfaction_rate: "required",
      },
      {},
    ),
  ),
  "get-status": body<DaemonStatus>(
    "DaemonStatus",
    shape<DaemonStatus>(
      {
        busy: "optional",
        db_path: "required",
        home_root: "required",
        llm: "optional",
        log_path: "required",
        produced_bytes: "required",
        project_root: "required",
        retention: "required",
        routing: "required",
        schema_version: "required",
        sessions: "required",
        started_at: "required",
        state_dir: "required",
        upload_bytes: "required",
        uptime_secs: "required",
      },
      {
        busy: shape<Inner<DaemonStatus["busy"]>>(
          {
            connected_clients: "required",
            pending_confirmations: "required",
            running_tasks: "required",
          },
          {},
        ),
        llm: shape<Inner<DaemonStatus["llm"]>>(
          {
            default_model: "required",
            default_model_routable: "required",
            effective_default_model: "required",
          },
          {},
        ),
        retention: shape<Inner<DaemonStatus["retention"]>>(
          {
            log_max_session_bytes: "required",
            log_max_total_bytes: "required",
            log_retention_days: "required",
          },
          {},
        ),
        routing: shape<Inner<DaemonStatus["routing"]>>(
          { resume_enabled: "required" },
          {},
        ),
        sessions: shape<Inner<DaemonStatus["sessions"]>>(
          { dropped_records: "required", last_sweep: "required" },
          {
            last_sweep: shape<
              Inner<Inner<DaemonStatus["sessions"]>["last_sweep"]>
            >(
              {
                bytes_after: "required",
                bytes_before: "required",
                bytes_freed: "required",
                files_removed: "required",
                index_rows_cleared: "required",
                over_cap_after: "required",
                sessions_evicted: "required",
                sessions_visited: "required",
              },
              {},
            ),
          },
        ),
      },
    ),
  ),
  "get-tasks": body<Task[]>(
    "Task[]",
    shape<Task>(
      {
        artifact_count: "optional",
        completed_at: "required",
        cost_usd: "optional",
        created_at: "required",
        created_by: "required",
        description: "required",
        id: "required",
        outcome: "optional",
        outcome_kind: "optional",
        priority: "required",
        progress_current: "required",
        progress_total: "required",
        result_summary: "required",
        source_lane: "required",
        state_version: "required",
        status: "required",
        steerable: "optional",
        subagent_count: "optional",
        title: "required",
        updated_at: "required",
      },
      {
        outcome: parsedOutcomeShape,
      },
    ),
  ),
  "get-tasks-id": body<TaskDetailResponse>(
    "TaskDetailResponse",
    shape<TaskDetailResponse>(
      { outcome: "optional", steerable: "optional", task: "required" },
      {
        outcome: parsedOutcomeShape,
        task: shape<Inner<TaskDetailResponse["task"]>>(
          {
            // The list row's only: the detail sends its outcome beside `task`.
            outcome: "may_omit",
            // The list row's only (GAP-08b).
            cost_usd: "may_omit",
            // The list row's only.
            subagent_count: "may_omit",
            // The list row's only: the detail sends it beside `task`.
            steerable: "may_omit",
            artifact_count: "optional",
            completed_at: "required",
            created_at: "required",
            created_by: "required",
            description: "required",
            id: "required",
            outcome_kind: "optional",
            priority: "required",
            progress_current: "required",
            progress_total: "required",
            result_summary: "required",
            source_lane: "required",
            state_version: "required",
            status: "required",
            title: "required",
            updated_at: "required",
          },
          {
            outcome: parsedOutcomeShape,
          },
        ),
      },
    ),
  ),
  "get-tasks-id-timeline": body<TaskTimeline>(
    "TaskTimeline",
    shape<TaskTimeline>(
      {
        completed_at: "required",
        lanes: "required",
        now: "required",
        started_at: "required",
        task_id: "required",
      },
      {
        lanes: shape<Inner<TaskTimeline["lanes"]>>(
          {
            // skip_serializing_if: nothing counts a subagent's steps (TimelineLaneResponse).
            steps_current: "may_omit",
            // skip_serializing_if: nothing counts a subagent's steps (TimelineLaneResponse).
            steps_total: "may_omit",
            agent_instance_id: "required",
            detail: "required",
            ended_at: "required",
            label: "required",
            lane_id: "required",
            started_at: "required",
            state: "required",
            template_id: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-tools": body<ToolCatalogEntry[]>(
    "ToolCatalogEntry[]",
    shape<ToolCatalogEntry>(
      {
        author: "required",
        description: "required",
        invocations_today: "required",
        name: "required",
        origin: "required",
        provides_capabilities: "required",
        requires_confirmation: "required",
        source: "required",
        version: "required",
      },
      {
        origin: shape<Inner<ToolCatalogEntry["origin"]>>(
          {
            enabled: "required",
            id: "required",
            kind: "required",
            state: "required",
          },
          {},
        ),
      },
    ),
  ),
  "get-usage-summary": body<UsageSummary>(
    "UsageSummary",
    shape<UsageSummary>(
      {
        by_provider: "required",
        caps: "required",
        date: "required",
        total_usd: "required",
      },
      {
        by_provider: shape<Inner<UsageSummary["by_provider"]>>(
          {
            calls: "required",
            provider: "required",
            tokens: "required",
            usd: "required",
          },
          {},
        ),
        caps: shape<Inner<UsageSummary["caps"]>>(
          { agent_max_cost_usd: "required", workflow_max_cost_usd: "required" },
          {},
        ),
      },
    ),
  ),
  "get-workspaces": body<Project[]>(
    "Project[]",
    shape<Project>(
      {
        id: "required",
        missing: "required",
        name: "required",
        root: "required",
      },
      {},
    ),
  ),
  "get-workspaces-path": body<Workspace>(
    "Workspace",
    shape<Workspace>(
      {
        active_tasks: "required",
        id: "required",
        missing: "required",
        name: "required",
        path: "required",
        queued_tasks: "required",
        root: "required",
        rows: "required",
      },
      {
        rows: shape<Inner<Workspace["rows"]>>(
          {
            artifacts: "required",
            followups: "required",
            messages: "required",
            sessions: "required",
            tasks: "required",
          },
          {},
        ),
      },
    ),
  ),
  "patch-sessions-id": body<Session>(
    "Session",
    shape<Session>(
      {
        active_task_count: "required",
        created_at: "required",
        ended_at: "required",
        id: "required",
        interrupted_task_count: "required",
        lane_key: "required",
        last_message_at: "required",
        message_count: "required",
        source: "required",
        status: "required",
        title: "required",
        updated_at: "required",
        working_state: "required",
        working_state_updated_at: "required",
        workspace_id: "required",
      },
      {
        working_state: workingStateShape,
      },
    ),
  ),
  "patch-workspaces": body<ProjectRepoint>(
    "ProjectRepoint",
    shape<ProjectRepoint>(
      {
        moved: "required",
        new_path: "required",
        old_path: "required",
        project: "required",
      },
      {
        moved: shape<Inner<ProjectRepoint["moved"]>>(
          {
            artifacts: "required",
            followups: "required",
            messages: "required",
            sessions: "required",
            tasks: "required",
          },
          {},
        ),
        project: shape<Inner<ProjectRepoint["project"]>>(
          {
            id: "required",
            missing: "required",
            name: "required",
            root: "required",
          },
          {},
        ),
      },
    ),
  ),
  "post-agent-templates": body<Awaited<ReturnType<typeof createAgentTemplate>>>(
    "createAgentTemplate's answer",
    shape<Inner<Awaited<ReturnType<typeof createAgentTemplate>>>>(
      {
        template_id: "required",
        overrides_bundled: "required",
        status: "required",
      },
      {},
    ),
  ),
  "post-agents-id-action": body<AgentActionResponse>(
    "AgentActionResponse",
    shape<AgentActionResponse>(
      { agent_id: "required", status: "required" },
      {},
    ),
  ),
  // `lib/chat-stream.ts`'s `sendChatMessage` (CONTRACT-SAMPLES-2).
  "post-chat": body<ChatSendResponse>(
    "ChatSendResponse",
    shape<ChatSendResponse>(
      { lane_key: "required", model_used: "required", stream_id: "required" },
      {},
    ),
  ),
  "post-command": body<ShutdownAccepted>(
    "ShutdownAccepted",
    shape<ShutdownAccepted>({ request_id: "required", status: "required" }, {}),
  ),
  "post-extensions-kind-id-verb": body<ExtensionRow>(
    "ExtensionRow",
    shape<ExtensionRow>(
      {
        // Per-call: only a verb that produced a warning sends it (row_json).
        warnings: "may_omit",
        actionable: "required",
        added_capabilities: "required",
        agents: "required",
        connect_ms: "optional",
        connector: "required",
        consent: "required",
        declared: "required",
        detail: "required",
        enabled: "required",
        hint: "required",
        id: "required",
        kind: "required",
        list_ms: "optional",
        missing_config_keys: "required",
        protocol_version: "optional",
        provider: "required",
        reason: "required",
        since: "required",
        skills: "required",
        skipped_tools: "required",
        state: "required",
        tools: "required",
        tools_changed_at: "required",
        transport: "required",
        version: "required",
        withdrawn_by_server: "required",
      },
      {
        declared: shape<Inner<ExtensionRow["declared"]>>(
          {
            capabilities: "required",
            types: "required",
            virtual_capabilities: "required",
            warnings: "optional",
          },
          {
            types: "opaque",
          },
        ),
      },
    ),
  ),
  "post-extensions-mcp": body<InstallResponse>(
    "InstallResponse",
    shape<InstallResponse>(
      {
        // A plugin install's only: an MCP server's answer is {extension, manifest}.
        added_capabilities: "may_omit",
        // A plugin install's only: an MCP server's answer is {extension, manifest}.
        consent_reset: "may_omit",
        extension: "required",
        manifest: "required",
      },
      {
        manifest: shape<Inner<InstallResponse["manifest"]>>(
          {
            name: "required",
            version: "required",
            description: "required",
            entry: "required",
            capabilities: "required",
            virtual_capabilities: "required",
            types: "required",
            required_config_keys: "required",
            sensitive_config_keys: "required",
            warnings: "optional",
          },
          {
            types: mapOf<Record<string, boolean>>("opaque"),
          },
        ),
        extension: shape<Inner<InstallResponse["extension"]>>(
          {
            // Per-call: only a verb that produced a warning sends it (row_json).
            warnings: "may_omit",
            actionable: "required",
            added_capabilities: "required",
            agents: "required",
            connect_ms: "optional",
            connector: "required",
            consent: "required",
            declared: "required",
            detail: "required",
            enabled: "required",
            hint: "required",
            id: "required",
            kind: "required",
            list_ms: "optional",
            missing_config_keys: "required",
            protocol_version: "optional",
            provider: "required",
            reason: "required",
            since: "required",
            skills: "required",
            skipped_tools: "required",
            state: "required",
            tools: "required",
            tools_changed_at: "required",
            transport: "required",
            version: "required",
            withdrawn_by_server: "required",
          },
          {
            declared: shape<
              Inner<Inner<InstallResponse["extension"]>["declared"]>
            >(
              {
                capabilities: "required",
                virtual_capabilities: "required",
                types: "required",
                warnings: "optional",
              },
              { types: mapOf<Record<string, boolean>>("opaque") },
            ),
          },
        ),
      },
    ),
  ),
  "post-extensions-plugin": body<InstallResponse>(
    "InstallResponse",
    shape<InstallResponse>(
      {
        added_capabilities: "optional",
        consent_reset: "optional",
        extension: "required",
        manifest: "required",
      },
      {
        extension: shape<Inner<InstallResponse["extension"]>>(
          {
            // Per-call: only a verb that produced a warning sends it (row_json).
            warnings: "may_omit",
            actionable: "required",
            added_capabilities: "required",
            agents: "required",
            connect_ms: "optional",
            connector: "required",
            consent: "required",
            declared: "required",
            detail: "required",
            enabled: "required",
            hint: "required",
            id: "required",
            kind: "required",
            list_ms: "optional",
            missing_config_keys: "required",
            protocol_version: "optional",
            provider: "required",
            reason: "required",
            since: "required",
            skills: "required",
            skipped_tools: "required",
            state: "required",
            tools: "required",
            tools_changed_at: "required",
            transport: "required",
            version: "required",
            withdrawn_by_server: "required",
          },
          {
            declared: shape<
              Inner<Inner<InstallResponse["extension"]>["declared"]>
            >(
              {
                capabilities: "required",
                types: "required",
                virtual_capabilities: "required",
                warnings: "optional",
              },
              {
                types: "opaque",
              },
            ),
          },
        ),
        manifest: shape<Inner<InstallResponse["manifest"]>>(
          {
            capabilities: "required",
            description: "required",
            entry: "required",
            name: "required",
            required_config_keys: "required",
            sensitive_config_keys: "required",
            types: "required",
            version: "required",
            virtual_capabilities: "required",
            warnings: "optional",
          },
          {
            types: "opaque",
          },
        ),
      },
    ),
  ),
  "post-extensions-plugin-validate": body<ValidateResponse>(
    "ValidateResponse",
    shape<ValidateResponse>(
      { installed: "required", manifest: "required" },
      {
        manifest: shape<Inner<ValidateResponse["manifest"]>>(
          {
            capabilities: "required",
            description: "required",
            entry: "required",
            name: "required",
            required_config_keys: "required",
            sensitive_config_keys: "required",
            types: "required",
            version: "required",
            virtual_capabilities: "required",
            warnings: "optional",
          },
          {
            types: "opaque",
          },
        ),
      },
    ),
  ),
  "post-files-id-open": body<FileOpenResponse>(
    "FileOpenResponse",
    shape<FileOpenResponse>({ id: "required", status: "required" }, {}),
  ),
  "post-files-upload": body<FileUploadResponse>(
    "FileUploadResponse",
    shape<FileUploadResponse>(
      {
        filename: "required",
        id: "required",
        mime_type: "required",
        size_bytes: "required",
        status: "required",
      },
      {},
    ),
  ),
  "post-lanes-key-followups": body<FollowupRecord>(
    "FollowupRecord",
    shape<FollowupRecord>(
      {
        content: "required",
        created_at: "required",
        id: "required",
        kind: "required",
        lane_key: "required",
        source_task_id: "required",
        status: "required",
        updated_at: "required",
      },
      {},
    ),
  ),
  "post-memories": body<MemoryStoreResult>(
    "MemoryStoreResult",
    shape<MemoryStoreResult>(
      {
        dedup: "required",
        id: "required",
        origin: "required",
        revision: "required",
        scope: "required",
        superseded: "required",
      },
      {},
    ),
  ),
  "post-memories-id-confirm": body<MemoryUpdateResult>(
    "MemoryUpdateResult",
    shape<MemoryUpdateResult>(
      {
        dismissed: "required",
        id: "required",
        revision: "required",
        status: "required",
        superseded: "required",
      },
      {},
    ),
  ),
  "post-memories-id-edit": body<MemoryUpdateResult>(
    "MemoryUpdateResult",
    shape<MemoryUpdateResult>(
      {
        dismissed: "required",
        id: "required",
        revision: "required",
        status: "required",
        superseded: "required",
      },
      {},
    ),
  ),
  "post-memories-id-undo": body<MemoryWithdrawn>(
    "MemoryWithdrawn",
    shape<MemoryWithdrawn>(
      { id: "required", restored: "required", status: "required" },
      {},
    ),
  ),
  "post-models-refresh": body<ModelEntry[]>("ModelEntry[]", modelEntryShape),
  "post-sessions": body<Session>(
    "Session",
    shape<Session>(
      {
        active_task_count: "required",
        created_at: "required",
        ended_at: "required",
        id: "required",
        interrupted_task_count: "required",
        lane_key: "required",
        last_message_at: "required",
        message_count: "required",
        source: "required",
        status: "required",
        title: "required",
        updated_at: "required",
        working_state: "required",
        working_state_updated_at: "required",
        workspace_id: "required",
      },
      {
        working_state: workingStateShape,
      },
    ),
  ),
  "post-sessions-id-activate": body<Session>(
    "Session",
    shape<Session>(
      {
        active_task_count: "required",
        created_at: "required",
        ended_at: "required",
        id: "required",
        interrupted_task_count: "required",
        lane_key: "required",
        last_message_at: "required",
        message_count: "required",
        source: "required",
        status: "required",
        title: "required",
        updated_at: "required",
        working_state: "required",
        working_state_updated_at: "required",
        workspace_id: "required",
      },
      {
        working_state: workingStateShape,
      },
    ),
  ),
  "post-sessions-id-archive": body<Session>(
    "Session",
    shape<Session>(
      {
        active_task_count: "required",
        created_at: "required",
        ended_at: "required",
        id: "required",
        interrupted_task_count: "required",
        lane_key: "required",
        last_message_at: "required",
        message_count: "required",
        source: "required",
        status: "required",
        title: "required",
        updated_at: "required",
        working_state: "required",
        working_state_updated_at: "required",
        workspace_id: "required",
      },
      {
        working_state: workingStateShape,
      },
    ),
  ),
  "post-settings-llm-credentials-rescan": body<DiscoveredCredentialInfo[]>(
    "DiscoveredCredentialInfo[]",
    shape<DiscoveredCredentialInfo>(
      {
        auto_refresh: "required",
        expires_at: "required",
        provider: "required",
        source: "required",
        status: "required",
      },
      {},
    ),
  ),
  "post-settings-llm-validate": body<KeyValidationResult>(
    "KeyValidationResult",
    shape<KeyValidationResult>(
      {
        detected_source: "required",
        format_error: "required",
        models_available: "required",
        rate_limits: "required",
        tier: "required",
        valid: "required",
      },
      {},
    ),
  ),
  "post-tasks": body<CreateTaskResponse>(
    "CreateTaskResponse",
    shape<CreateTaskResponse>({ status: "required", task_id: "required" }, {}),
  ),
  "post-tasks-id-action-cancel": body<TaskActionResponse>(
    "TaskActionResponse",
    shape<TaskActionResponse>(
      {
        // Resume's only (resume_task); a cancel answers {task_id, status}.
        session_id: "may_omit",
        // Resume's only (resume_task).
        rounds_replayed: "may_omit",
        // Resume's only (resume_task).
        from_seq: "may_omit",
        // Resume's only (resume_task).
        to_seq: "may_omit",
        status: "required",
        task_id: "required",
      },
      {},
    ),
  ),
  "post-tasks-id-action-resume": body<TaskActionResponse>(
    "TaskActionResponse",
    shape<TaskActionResponse>(
      {
        from_seq: "optional",
        rounds_replayed: "optional",
        session_id: "optional",
        status: "required",
        task_id: "required",
        to_seq: "optional",
      },
      {},
    ),
  ),
  "post-tasks-id-action-start": body<TaskActionResponse>(
    "TaskActionResponse",
    shape<TaskActionResponse>(
      {
        // Resume's only (resume_task); a start answers {task_id, status}.
        session_id: "may_omit",
        // Resume's only (resume_task).
        rounds_replayed: "may_omit",
        // Resume's only (resume_task).
        from_seq: "may_omit",
        // Resume's only (resume_task).
        to_seq: "may_omit",
        status: "required",
        task_id: "required",
      },
      {},
    ),
  ),
  "post-tasks-id-rerun": body<RerunResult>(
    "RerunResult",
    shape<RerunResult>(
      {
        source_task_id: "required",
        status: "required",
        task_id: "required",
        title: "required",
      },
      {},
    ),
  ),
  "post-tasks-id-steer": body<SteerResult>(
    "SteerResult",
    shape<SteerResult>(
      {
        accepted: "required",
        inbox_depth: "required",
        lane_key: "required",
        task_id: "required",
      },
      {},
    ),
  ),
  "post-workspaces": body<Project>(
    "Project",
    shape<Project>(
      {
        id: "required",
        missing: "required",
        name: "required",
        root: "required",
      },
      {},
    ),
  ),
  "post-workspaces-purge": body<ProjectDataDeletion>(
    "ProjectDataDeletion",
    shape<ProjectDataDeletion>(
      {
        applied: "required",
        counts: "required",
        dry_run: "required",
        entries: "required",
        kept: "required",
        project: "required",
        removed: "required",
      },
      {
        counts: shape<Inner<ProjectDataDeletion["counts"]>>(
          {
            artifacts: "required",
            followups: "required",
            memory_notes: "required",
            messages: "required",
            run_events: "required",
            sessions: "required",
            spans: "required",
            tasks: "required",
            tool_calls: "required",
            uploads: "required",
          },
          {},
        ),
        entries: shape<Inner<ProjectDataDeletion["entries"]>>(
          {
            action: "required",
            entry: "required",
            holds: "required",
            retention: "required",
          },
          {},
        ),
        kept: shape<Inner<ProjectDataDeletion["kept"]>>(
          { artifacts: "required", uploads: "required" },
          {},
        ),
        project: shape<Inner<ProjectDataDeletion["project"]>>(
          {
            id: "required",
            missing: "required",
            name: "required",
            root: "required",
          },
          {},
        ),
        removed: shape<Inner<ProjectDataDeletion["removed"]>>(
          {
            memory_partitions: "required",
            project_folder: "required",
            session_dirs: "required",
          },
          {},
        ),
      },
    ),
  ),
  "put-agent-templates-id": body<
    Awaited<ReturnType<typeof updateAgentTemplate>>
  >(
    "updateAgentTemplate's answer",
    shape<Inner<Awaited<ReturnType<typeof updateAgentTemplate>>>>(
      {
        template_id: "required",
        overrides_bundled: "required",
        status: "required",
      },
      {},
    ),
  ),
  "put-chat-messages-id-feedback": body<FeedbackResponse>(
    "FeedbackResponse",
    shape<FeedbackResponse>(
      { comment: "required", feedback: "required", message_id: "required" },
      {},
    ),
  ),
  "put-extensions-plugin-id": body<InstallResponse>(
    "InstallResponse",
    shape<InstallResponse>(
      {
        added_capabilities: "optional",
        consent_reset: "optional",
        extension: "required",
        manifest: "required",
      },
      {
        extension: shape<Inner<InstallResponse["extension"]>>(
          {
            // Per-call: only a verb that produced a warning sends it (row_json).
            warnings: "may_omit",
            actionable: "required",
            added_capabilities: "required",
            agents: "required",
            connect_ms: "optional",
            connector: "required",
            consent: "required",
            declared: "required",
            detail: "required",
            enabled: "required",
            hint: "required",
            id: "required",
            kind: "required",
            list_ms: "optional",
            missing_config_keys: "required",
            protocol_version: "optional",
            provider: "required",
            reason: "required",
            since: "required",
            skills: "required",
            skipped_tools: "required",
            state: "required",
            tools: "required",
            tools_changed_at: "required",
            transport: "required",
            version: "required",
            withdrawn_by_server: "required",
          },
          {
            declared: shape<
              Inner<Inner<InstallResponse["extension"]>["declared"]>
            >(
              {
                capabilities: "required",
                types: "required",
                virtual_capabilities: "required",
                warnings: "optional",
              },
              {
                types: "opaque",
              },
            ),
          },
        ),
        manifest: shape<Inner<InstallResponse["manifest"]>>(
          {
            capabilities: "required",
            description: "required",
            entry: "required",
            name: "required",
            required_config_keys: "required",
            sensitive_config_keys: "required",
            types: "required",
            version: "required",
            virtual_capabilities: "required",
            warnings: "optional",
          },
          {
            types: "opaque",
          },
        ),
      },
    ),
  ),
  "put-persona-user": body<{ etag: string }>(
    "{ etag: string }",
    shape<{ etag: string }>({ etag: "required" }, {}),
  ),
  "put-settings-llm-providers-provider-enabled": body<ProviderEnabledResponse>(
    "ProviderEnabledResponse",
    shape<ProviderEnabledResponse>(
      {
        discovered_models: "required",
        discovery_error: "required",
        enabled: "required",
        id: "required",
        loaded: "required",
        warning: "required",
      },
      {},
    ),
  ),
};

// ── The events ──────────────────────────────────────────────────────────────

/** A frame as the socket sends it: `_id` is stamped on receipt. */
type Frame<K extends ServerEvent["type"]> = Omit<
  Extract<ServerEvent, { type: K }>,
  "_id"
>;

/** The keys every frame carries besides its own. */
const common = {
  type: "required",
  ts: "required",
  instance_id: "required",
} as const;

/** Every variant of the union, each with its descriptor. */
const EVENTS: { readonly [K in ServerEvent["type"]]: Node<Frame<K>> } = {
  heartbeat: shape<Frame<"heartbeat">>({ ...common }, {}),
  daemon_shutting_down: shape<Frame<"daemon_shutting_down">>(
    { ...common, grace_secs: "required" },
    {},
  ),
  command_received: shape<Frame<"command_received">>(
    { ...common, request_id: "required", command: "required" },
    {},
  ),
  wake: shape<Frame<"wake">>({ ...common, wake: "required" }, {}),
  connector_status: shape<Frame<"connector_status">>(
    { ...common, id: "required", status: "required" },
    {},
  ),
  task_status: shape<Frame<"task_status">>(
    {
      ...common,
      task_id: "required",
      title: "required",
      status: "required",
      progress_current: "required",
      progress_total: "required",
      result_summary: "required",
      outcome_kind: "optional",
      artifact_count: "optional",
      outcome_summary: "optional",
    },
    {},
  ),
  agent_status: shape<Frame<"agent_status">>(
    {
      ...common,
      agent_id: "required",
      name: "required",
      status: "required",
      current_task_id: "required",
      agent_instance_id: "required",
      template_id: "required",
    },
    {},
  ),
  key_status_changed: shape<Frame<"key_status_changed">>(
    { ...common, provider: "required", key_id: "required", status: "required" },
    {},
  ),
  chat_stream_started: shape<Frame<"chat_stream_started">>(
    { ...common, stream_id: "required", lane_key: "required" },
    {},
  ),
  chat_stream_ended: shape<Frame<"chat_stream_ended">>(
    {
      ...common,
      stream_id: "required",
      lane_key: "required",
      status: "required",
    },
    {},
  ),
  agent_config_changed: shape<Frame<"agent_config_changed">>(
    {
      ...common,
      agent_id: "required",
      action: "required",
      config_version: "required",
    },
    {},
  ),
  orchestrator_config_changed: shape<Frame<"orchestrator_config_changed">>(
    { ...common, model: "required" },
    {},
  ),
  security_violation: shape<Frame<"security_violation">>(
    {
      ...common,
      agent_id: "required",
      tool_name: "required",
      reason: "required",
      task_id: "required",
    },
    {},
  ),
  circuit_breaker_tripped: shape<Frame<"circuit_breaker_tripped">>(
    {
      ...common,
      agent_id: "required",
      tool_name: "required",
      consecutive_failures: "required",
      reset_after_secs: "required",
      task_id: "required",
    },
    {},
  ),
  tool_executed: shape<Frame<"tool_executed">>(
    {
      ...common,
      agent_id: "required",
      tool_name: "required",
      success: "required",
      duration_ms: "required",
      task_id: "required",
      reason: "optional",
    },
    {},
  ),
  llm_call_completed: shape<Frame<"llm_call_completed">>(
    {
      ...common,
      agent_id: "required",
      model: "required",
      input_tokens: "required",
      output_tokens: "required",
      cost_usd: "required",
      task_id: "required",
    },
    {},
  ),
  skill_catalog_updated: shape<Frame<"skill_catalog_updated">>(
    { ...common, skill_name: "required", action: "required" },
    {},
  ),
  skill_invocation_started: shape<Frame<"skill_invocation_started">>(
    {
      ...common,
      request_id: "required",
      skill_id: "required",
      query_preview: "required",
    },
    {},
  ),
  skill_completed: shape<Frame<"skill_completed">>(
    {
      ...common,
      request_id: "required",
      skill_id: "required",
      duration_ms: "required",
      output_preview: "required",
    },
    {},
  ),
  skill_failed: shape<Frame<"skill_failed">>(
    {
      ...common,
      request_id: "required",
      skill_id: "required",
      error: "required",
    },
    {},
  ),
  tool_confirmation_requested: shape<Frame<"tool_confirmation_requested">>(
    {
      ...common,
      request_id: "required",
      agent_id: "required",
      tool_name: "required",
      tool_arguments: "required",
      stream_id: "required",
      lane_key: "required",
      task_id: "required",
      preview: "optional",
      per_call: "optional",
    },
    {},
  ),
  tool_confirmation_resolved: shape<Frame<"tool_confirmation_resolved">>(
    {
      ...common,
      request_id: "required",
      agent_id: "required",
      tool_name: "required",
      outcome: "required",
      stream_id: "required",
      lane_key: "required",
      task_id: "required",
    },
    {},
  ),
  soul_updated: shape<Frame<"soul_updated">>(
    {
      ...common,
      actor: "required",
      mode: "required",
      content_sha256: "required",
      backup_path: "required",
    },
    {},
  ),
  daemon_config_changed: shape<Frame<"daemon_config_changed">>(
    { ...common },
    {},
  ),
  workflow_started: shape<Frame<"workflow_started">>(
    { ...common, task_id: "required", lane_key: "required", title: "required" },
    {},
  ),
  workflow_steered: shape<Frame<"workflow_steered">>(
    { ...common, task_id: "required", lane_key: "required" },
    {},
  ),
  workflow_progress: shape<Frame<"workflow_progress">>(
    {
      ...common,
      task_id: "required",
      lane_key: "required",
      message: "required",
    },
    {},
  ),
  followup_queued: shape<Frame<"followup_queued">>(
    {
      ...common,
      lane_key: "required",
      followup_id: "required",
      kind: "required",
    },
    {},
  ),
  session_changed: shape<Frame<"session_changed">>(
    {
      ...common,
      session_id: "required",
      lane_key: "required",
      status: "required",
      task_id: "required",
    },
    {},
  ),
  followup_cancelled: shape<Frame<"followup_cancelled">>(
    { ...common, lane_key: "required", followup_id: "required" },
    {},
  ),
  artifact_written: shape<Frame<"artifact_written">>(
    {
      ...common,
      artifact_id: "required",
      task_id: "required",
      agent_id: "required",
      name: "required",
      kind: "required",
      version: "required",
      path: "required",
      in_project: "required",
      openable: "required",
    },
    {},
  ),
  subagent_span: shape<Frame<"subagent_span">>(
    {
      ...common,
      task_id: "required",
      span_id: "required",
      label: "required",
      template_id: "required",
      agent_instance_id: "required",
      state: "required",
      detail: "required",
      started_at: "required",
      ended_at: "required",
      duration_ms: "required",
      output_preview: "required",
    },
    {},
  ),
  extension_state_changed: shape<Frame<"extension_state_changed">>(
    {
      ...common,
      kind: "required",
      id: "required",
      state: "required",
      generation: "required",
      tools_changed: "required",
    },
    {},
  ),
  extension_capability_withheld: shape<Frame<"extension_capability_withheld">>(
    {
      ...common,
      kind: "required",
      id: "required",
      subject: "required",
      moment: "required",
      state: "required",
      scope: "required",
      stale: "required",
    },
    {},
  ),
  extension_capability_withdrawn: shape<
    Frame<"extension_capability_withdrawn">
  >(
    {
      ...common,
      kind: "required",
      id: "required",
      state: "required",
      cause: "required",
      capabilities: "required",
      tools: "required",
      affected_templates: "required",
      affected_skills: "required",
      affected_cron_skills: "required",
      notice_lane: "required",
    },
    {},
  ),
  memory_changed: shape<Frame<"memory_changed">>(
    {
      ...common,
      partition_key: "required",
      note_id: "required",
      change: "required",
      revision: "required",
      origin: "required",
      surface: "optional",
    },
    {},
  ),
};

// ── The checks ──────────────────────────────────────────────────────────────

function sorted(names: Iterable<string>): string[] {
  return [...names].sort();
}

describe("the API contract's samples", () => {
  it("every response sample has its type here, and every type its sample", () => {
    expect(sorted(responseSamples.keys())).toEqual(
      sorted(Object.keys(RESPONSES)),
    );
  });

  it("every event sample is a variant of the union, and every variant has its sample", () => {
    expect(sorted(eventSamples.keys())).toEqual(sorted(Object.keys(EVENTS)));
  });

  it.each(sorted(Object.keys(RESPONSES)))(
    "responses/%s parses as the type the GUI reads it as",
    (name) => {
      const entry = RESPONSES[name];
      const sample = responseSamples.get(name);
      if (entry === undefined || sample === undefined) return;
      expect(
        check(sample, entry.node),
        `responses/${name}.json read as ${entry.type}`,
      ).toEqual([]);
    },
  );

  // CONTRACT-SAMPLES-2 (*Rev 107*'s renamed-optional-key item, (b)): an
  // optional key the sample lacks fails unless its descriptor marks it one
  // the daemon may omit — the health answer's `protocol`, the key Q04's
  // compatibility check reads, renamed as the post-review saw it.
  it("a renamed optional key fails, and only a may_omit mark lets one be absent", () => {
    const entry = RESPONSES["get-health"];
    const sample = responseSamples.get("get-health") as Record<string, unknown>;
    expect(entry).toBeDefined();
    if (entry === undefined) return;
    const { protocol, ...rest } = sample;
    const renamed = { ...rest, wire_protocol: protocol };
    expect(check(renamed, entry.node)).toEqual([
      'protocol: optional in the type, absent from the sample (renamed or removed, or a key the daemon may omit: mark it "may_omit")',
    ]);
    const lenient = shape<{ a: string; b?: number }>(
      { a: "required", b: "may_omit" },
      {},
    );
    expect(check({ a: "" }, lenient)).toEqual([]);
  });

  it.each(sorted(Object.keys(EVENTS)))(
    "events/%s parses as its ServerEvent variant",
    (name) => {
      const node = EVENTS[name as ServerEvent["type"]] as Node<unknown>;
      const sample = eventSamples.get(name);
      if (sample === undefined) return;
      expect((sample as { type?: unknown }).type).toBe(name);
      expect(
        check(sample, node),
        `events/${name}.json read as ServerEvent "${name}"`,
      ).toEqual([]);
    },
  );
});
