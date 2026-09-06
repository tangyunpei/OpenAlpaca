/**
 * Daemon wire types.
 *
 * Every shape here is transcribed from the Rust source the daemon actually
 * serializes (see `API_MAP.md` §"Sources verified"), not from the design
 * fixtures. Where the legacy SvelteKit client drifted from the daemon the
 * daemon wins — e.g. agent templates serialize `capabilities` /
 * `denied_capabilities`, which the old client called `skills` / `denied_skills`.
 *
 * Field names stay snake_case because that is what crosses the wire; only the
 * Tauri `ConnectionInfo` is camelCase (serde rename on the Rust side).
 */

// ── Tasks ───────────────────────────────────────────────────────────────────

/** `TaskStatus` on the wire (`apps/openalpacad/src/routes/tasks_types.rs`). */
export type TaskStatusValue =
  "queued" | "running" | "paused" | "completed" | "failed" | "cancelled";

/** The design's five-state run model. `completed` maps to `done`. */
export type RunStatus =
  "running" | "queued" | "paused" | "done" | "cancelled" | "failed";

/** Free-form artifact reference parsed out of `task.outcome_json`. Schema-less by design; `/v1/artifacts?task_id=` is the typed answer. */
export interface ParsedOutcome {
  outcome_summary: string | null;
  outcome_kind: string;
  artifact_count: number;
  artifacts: unknown[];
  no_artifact_reason?: string;
}

/** Serialized `Task`. `state_json`/`outcome_json` are `#[serde(skip_serializing)]`. */
export interface Task {
  id: string;
  title: string;
  description: string | null;
  status: TaskStatusValue;
  priority: number;
  progress_current: number | null;
  progress_total: number | null;
  result_summary: string | null;
  created_by: string;
  source_lane: string;
  created_at: string;
  updated_at: string;
  completed_at: string | null;
  state_version: number;
  outcome_kind?: string;
  artifact_count?: number;
  outcome?: ParsedOutcome;
  /**
   * List route only (GAP-08b) — one grouped `cost_for_tasks` query per page,
   * `0` for a task with no logged LLM calls. The detail route's `Task` has no
   * such field yet; that unification is a later phase (see `run-model.ts`).
   */
  cost_usd?: number;
  /**
   * List route only (R38) — how many agents the run spawned, from one grouped
   * `subagent_span` query per page. This is what is left of the
   * `assigned_agents` array P8 deleted: a count, not names. A span opens at
   * spawn, so an agent still working is counted. Absent on the detail route,
   * and on a daemon older than the field.
   */
  subagent_count?: number;
  /**
   * R40 — whether `POST /v1/tasks/{id}/steer` would take a message for this
   * run right now: it is the local user's, it is not terminal, and a workflow
   * is still attached to it. Computed at read time, never stored, and served
   * by both task routes (on the detail it sits beside `task`, not inside it).
   *
   * Absent on a daemon older than the field — which is not the same as
   * `false`, so treat only an explicit `false` as a refusal.
   */
  steerable?: boolean;
}

/**
 * `GET /v1/tasks/{id}`. Still a different shape from a list row (nested under
 * `task`, no `cost_usd`) — API_MAP §5 warns the two disagree; do not conflate
 * them.
 *
 * The legacy `assignments` array is gone (P8): a run's agents are lanes on
 * `GET /v1/tasks/{id}/timeline`, which — unlike `agent_task_history` — has a
 * row for a subagent that is still working.
 */
export interface TaskDetailResponse {
  task: Task;
  outcome?: ParsedOutcome;
  /**
   * R40's steerability hint. It sits beside `task` rather than inside it
   * because it is not one of the run's columns — see {@link Task.steerable}.
   */
  steerable?: boolean;
}

export interface CreateTaskRequest {
  title: string;
  description?: string;
  priority?: number;
  created_by: string;
  source_lane: string;
}

export interface CreateTaskResponse {
  task_id: string;
  status: string;
}

export interface TaskActionResponse {
  task_id: string;
  status: string;
}

/**
 * The verbs `POST /v1/tasks/{id}/action` accepts.
 *
 * The first three are state transitions. `start` is not: it dispatches a
 * *stored* row — one `POST /v1/tasks` queued and nothing ever ran — under its
 * own id (D5), which is why it answers with the id you sent. Re-running a
 * finished run is a different route, `POST /v1/tasks/{id}/rerun`, because that
 * one answers with an id you have not seen.
 */
export type TaskAction = "cancel" | "pause" | "resume" | "start";

// ── Chat ────────────────────────────────────────────────────────────────────

export interface AttachmentRef {
  file_id: string;
  caption?: string;
}

export interface AttachmentDisplay {
  file_id: string;
  filename: string;
  mime_type: string;
  size_bytes: number;
}

export interface ToolConfirmation {
  request_id: string;
  tool_name: string;
  tool_arguments: unknown;
  status: "pending" | "approved" | "denied" | "expired";
}

/**
 * One `role='artifact'` link on a stored message (GAP-23, closed): a file the
 * message's *run* produced, as the daemon resolves it — the id the Library
 * opens, the name to show, and the kind the badge is drawn from.
 */
export interface MessageArtifact {
  id: string;
  name: string;
  /** The daemon's snake_case `ArtifactKind`; `null` for a pre-036 row. */
  kind: string | null;
}

/** `ConversationMessageView` — the stored row, plus its artifact links. */
export interface ChatMessage {
  id: number;
  lane_key: string;
  role: "user" | "assistant" | "system";
  content: string;
  source?: string;
  model?: string;
  tokens_in?: number;
  tokens_out?: number;
  duration_ms?: number;
  created_at: string;
  attachments?: AttachmentDisplay[];
  content_json?: string | null;
  display_text?: string | null;
  confirmation?: ToolConfirmation;
  /**
   * The run this message started (a delegating turn) or reported on (a
   * completion report) — migration 038's column. `null` for ordinary chat.
   */
  task_id?: string | null;
  /**
   * Files this message's run produced. Served on every message (`[]` for the
   * overwhelming majority), so no client has to tell "none" from "not served".
   */
  artifacts?: MessageArtifact[];
}

export interface ChatSendRequest {
  content: string;
  attachments?: AttachmentRef[];
  /**
   * Per-request model override — GAP-13. The daemon ignores unknown fields
   * (serde default), so sending it keeps the client honest and
   * forward-compatible without changing daemon behaviour today.
   */
  model?: string;
}

export interface ChatSendResponse {
  stream_id: string;
  lane_key: string;
}

export interface ChatHistoryResponse {
  messages: ChatMessage[];
  total: number;
  lane_key: string;
}

export interface ChatDeleteResponse {
  deleted: number;
}

/**
 * `ApprovalScope` (`security/confirmation.rs`). Forwarded by the HTTP route
 * to the sandbox's `ConfirmationResponse` (GAP-01, closed): an omitted or
 * `these_args` scope approves this call only; `entire_tool` caches the
 * approval for the rest of the session.
 */
export type ApprovalScope = "these_args" | "entire_tool";

export interface ConfirmationRequestBody {
  approved: boolean;
  approval_scope?: ApprovalScope;
}

export type FeedbackValue = "positive" | "negative";

export interface FeedbackResponse {
  message_id: number;
  feedback: FeedbackValue;
  comment: string | null;
}

// ── Sessions ────────────────────────────────────────────────────────────────
//
// A session is one conversation transcript: an epoch of a lane, bound to at
// most one workspace, with an `active` → `archived` lifecycle. A lane holds
// many of them and at most one active one. `/v1/sessions` replaced the two
// `/v1/conversations` reads (daemon plan §5.7, P19); the compaction columns
// (`summary`, `summary_version`, …) are the daemon's own bookkeeping and are
// deliberately not on the wire.

export type SessionStatus = "active" | "archived";

export interface Session {
  id: string;
  lane_key: string;
  source: string;
  title: string;
  workspace_id: string | null;
  status: SessionStatus;
  message_count: number;
  last_message_at: string | null;
  created_at: string;
  updated_at: string;
  ended_at: string | null;
  /** Runs this session's lane has in flight right now. */
  active_task_count: number;
  /** Runs started from this session that were left interrupted. */
  interrupted_task_count: number;
}

export interface SessionsResponse {
  sessions: Session[];
  total: number;
}

export interface SessionMessagesResponse {
  messages: ChatMessage[];
  total: number;
}

// ── Files ───────────────────────────────────────────────────────────────────

export type FileAssetStatus = "uploaded" | "processing" | "ready" | "error";

export interface FileAsset {
  id: string;
  owner_id: string;
  sha256: string;
  filename: string;
  mime_type: string;
  size_bytes: number;
  storage_path: string;
  status: FileAssetStatus;
  extracted_text: string | null;
  extract_error: string | null;
  metadata_json: string | null;
  created_at: string;
  updated_at: string;
}

export interface FileUploadResponse {
  id: string;
  filename: string;
  mime_type: string;
  size_bytes: number;
  status: string;
}

export interface FileOpenResponse {
  id: string;
  status: "opened";
}

// ── LLM settings, keys, models ──────────────────────────────────────────────

export type KeyPriorityValue = "primary" | "fallback";
export type KeySourceValue =
  | "api_console"
  | "claude_code"
  | "claude_max_pro"
  | "codex"
  | "environment"
  | "other";
export type KeyHealthValue = "healthy" | "rate_limited" | "error" | "unknown";

export interface ExternalUsage {
  period: string;
  cost_usd: number;
  token_count: number;
  rate_limit_remaining: number | null;
  fetched_at: string;
  approximate: boolean;
}

export interface KeyInfo {
  id: string;
  masked_secret: string;
  tier: string | null;
  priority: KeyPriorityValue;
  source: KeySourceValue;
  notes: string | null;
  status: string;
  monthly_usage_usd: number | null;
  managed?: boolean;
  credential_status?: string | null;
  credential_expires_at?: number | null;
  external_usage?: ExternalUsage | null;
}

export interface ProviderInfo {
  enabled: boolean;
  key_selection_strategy: string;
  keys: KeyInfo[];
}

export interface OrchestratorInfo {
  model: string;
  fallback_models: string[];
}

export interface LlmSettingsResponse {
  orchestrator: OrchestratorInfo;
  providers: Record<string, ProviderInfo>;
}

export interface KeyStatusEntry {
  id: string;
  health: KeyHealthValue;
  consecutive_rate_limits: number;
  is_available: boolean;
}

export type KeyStatusMap = Record<string, KeyStatusEntry[]>;

export interface AddKeyRequest {
  provider: string;
  key: {
    id?: string;
    secret: string;
    tier?: string;
    priority?: string;
    source?: string;
    notes?: string;
  };
}

export interface ReorderKeysRequest {
  provider: string;
  key_order: string[];
  primary_key_id?: string;
}

export interface SetKeyPriorityRequest {
  provider: string;
  key_id: string;
  priority: KeyPriorityValue;
}

export interface ValidateKeyRequest {
  provider: string;
  secret: string;
}

export interface KeyValidationResult {
  valid: boolean;
  tier: string | null;
  detected_source: string | null;
  models_available: string[];
  rate_limits: string | null;
  format_error: string | null;
}

export interface DiscoveredCredentialInfo {
  source: "claude_code" | "codex";
  provider: string;
  status: string;
  expires_at: number | null;
  auto_refresh: boolean;
}

export interface CliBackendStatus {
  name: string;
  available: boolean;
  path: string | null;
  enabled: boolean;
}

/** `health` is hardcoded `"healthy"`; `total_tokens` is lifetime, not today (GAP-08c). */
export interface ProviderUsageSummary {
  provider: string;
  total_cost_usd: number;
  total_tokens: number;
  total_requests: number;
  health: string;
  external_usage: ExternalUsage | null;
}

/** `routing::model_registry::ModelEntry`. */
export interface ModelEntry {
  id: string;
  provider: string;
  context_window: number;
  input_price_per_million: number;
  output_price_per_million: number;
}

// ── Usage ───────────────────────────────────────────────────────────────────

export interface LlmCallLog {
  id: number;
  timestamp: string;
  agent_id: string | null;
  task_id: string | null;
  provider: string;
  model: string;
  key_id: string | null;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  status: string;
  latency_ms: number | null;
  error_message: string | null;
}

export interface LlmUsageDaily {
  date: string;
  agent_id: string;
  model: string;
  total_requests: number;
  total_input_tokens: number;
  total_output_tokens: number;
  total_cost_usd: number;
}

// ── Orchestrator ────────────────────────────────────────────────────────────

/** `daily_cost_usd` sums today's UTC `llm_usage_daily` rows (GAP-08a, closed). */
export interface OrchestratorConfigResponse {
  model: string;
  fallback_models: string[];
  active_agents: number;
  active_tasks: number;
  daily_cost_usd: number;
}

export interface UpdateOrchestratorRequest {
  model: string;
  fallback_models: string[];
}

export interface OrchestratorLatencyRecord {
  id: number;
  request_id: string;
  mode: string;
  ack_ms: number;
  fallback_reason: string | null;
  auto_promotion_reason: string | null;
  timestamp: string;
}

export interface LatencyAggregate {
  mode: string;
  count: number;
  p50_total_ms: number;
  p95_total_ms: number;
  p99_total_ms: number;
  mean_ack_ms: number;
  auto_promotion_count: number;
  fallback_count: number;
}

export interface DispatchDecisionRecord {
  id: number;
  request_id: string;
  task_id: string | null;
  mode: string;
  reason: string;
  agent_count: number;
  dag_node_count: number | null;
  predictability_score: number | null;
  error_message: string | null;
  timestamp: string;
}

// ── Agents ──────────────────────────────────────────────────────────────────

/** `TemplateResponse` — note `capabilities`, not `skills`. */
export interface AgentTemplate {
  id: string;
  name: string;
  description: string;
  icon?: string;
  singleton: boolean;
  capabilities: string[];
  denied_capabilities: string[];
  temperature: number;
  verbosity: string;
  model?: string;
  fallback_models: string[];
  max_tool_calls?: number;
  timeout_seconds?: number;
  max_cost_per_task?: number;
  require_confirmation_for: string[];
  persona: string;
  body: string;
  /**
   * Lifetime runs of this template (GAP-20, counts half), counted from
   * `subagent_span` — one row per spawned subagent, opened at spawn time, so
   * a run still in flight is included. Always sent; `0` for a template
   * nothing has spawned.
   */
  run_count: number;
  /** When the newest of those runs started. Absent when there are none. */
  last_run_at?: string;
}

export interface AgentInstance {
  id: string;
  template_id: string;
  name: string;
  status: string;
  current_task: string | null;
}

export interface Agent {
  id: string;
  name: string;
  description: string | null;
  icon: string | null;
  status: string;
  current_task_id: string | null;
  template_id?: string;
  skills_json: string;
  preset_json: string;
  constraints_json: string | null;
  llm_config_json: string | null;
  persona: string | null;
  created_at: string;
  updated_at: string | null;
}

/** Lifetime-scoped and keyed by instance, not template — GAP-20. */
export interface AgentMetrics {
  agent_id: string;
  tasks_completed: number;
  tasks_failed: number;
  total_runtime_seconds: number;
  average_runtime_seconds: number;
  success_rate: number;
  updated_at: string;
}

export interface AgentDetailResponse {
  agent: Agent;
  metrics: AgentMetrics | null;
}

export interface AgentActionResponse {
  agent_id: string;
  status: string;
}

export interface AgentConfigFile {
  agent: { id: string; name: string; description: string; icon?: string };
  skills: { assigned: string[]; denied?: string[] };
  preset: { persona: string; temperature?: number; verbosity?: string };
  constraints?: {
    max_tool_calls?: number;
    timeout_seconds?: number;
    max_cost_per_task?: number;
    require_confirmation_for?: string[];
    allowed_capabilities?: string[];
    denied_capabilities?: string[];
  };
  llm?: { model?: string; fallback_models?: string[] };
}

export interface AgentConfigResponse {
  config: AgentConfigFile;
  config_version: number;
}

// ── Extensions / tools / connectors / skills ────────────────────────────────

/** The two ENABLE-axis extension kinds (ADR-030 §1). */
export type ExtensionKind = "mcp" | "plugin";

/** `ExtensionState::word()` — reported literally, never as a target state. */
export type ExtensionStateWord =
  | "enabled"
  | "disabled"
  | "unapproved"
  | "failed"
  | "orphaned"
  | "enabling"
  | "disabling";

/** `UnapprovedReason::word()` ∪ `FailureReason::word()` (ADR-030 §8). */
export type ExtensionReason =
  | "never_seen"
  | "denied"
  | "capabilities_grew"
  | "needs_authorization"
  | "needs_config"
  | "config_invalid"
  | "unreachable"
  | "crashed";

export type ExtensionConsent = "approved" | "pending" | "denied";

/**
 * What `plugin.toml` declares, read at scan — **static**, never a cache of
 * runtime discovery (ADR-030 §8, X-19). It is what an `unapproved` row shows,
 * because a plugin that has never run has no runtime `tools` to show.
 */
export interface DeclaredContributions {
  capabilities: string[];
  virtual_capabilities: string[];
  /** `plugin.toml`'s `[types]` table, as declared. */
  types: Record<string, boolean>;
}

/** One row of `GET /v1/extensions` (ADR-030 §8). */
export interface ExtensionRow {
  kind: ExtensionKind;
  id: string;
  version: string | null;
  /** MCP only — `stdio` | `streamable-http`. */
  transport: string | null;
  /**
   * The **persisted disposition** — the toggle binds here, never to the state
   * word. `null` on the two rows whose bit nobody can read (§4, §8): a plugin
   * while `.permissions.toml` is unreadable, and the `config/mcp.toml`
   * pseudo-record.
   */
  enabled: boolean | null;
  /** Plugins only; `null` for MCP. */
  consent: ExtensionConsent | null;
  state: ExtensionStateWord;
  reason: ExtensionReason | null;
  /** `FailureReason::actionable()` — drives the tone and the CTA (§9.2). */
  actionable: boolean;
  detail: string | null;
  hint: string | null;
  missing_config_keys: string[];
  /** `Unapproved{CapabilitiesGrew}` — the DELTA, not the whole list. */
  added_capabilities: string[];
  /** Live when `enabled`; empty otherwise — never a cache (§10). */
  tools: string[];
  skipped_tools: string[];
  withdrawn_by_server: string[];
  tools_changed_at: string | null;
  declared: DeclaredContributions | null;
  skills: string[];
  agents: string[];
  connector: string | null;
  provider: string | null;
  /** When the record entered its **current** state — every state, not just failed. */
  since: string;
  /** Present only on the verb that produced one (`disable` / `reload`). */
  warnings?: string[];
}

export type ExtensionVerb =
  "enable" | "disable" | "reload" | "approve" | "deny";

/**
 * Where an extension tool came from (`GET /v1/tools`). `null` for builtins and
 * for `config/tools/*.toml` tools — a builtin row carries no enable field at
 * all, because there is no per-tool enable state anywhere (S1, §8).
 */
export interface ToolOrigin {
  kind: ExtensionKind;
  id: string;
  enabled: boolean;
  state: ExtensionStateWord;
}

/** One row of `GET /v1/tools` (ADR-030 §8, GAP-18's tool half). */
export interface ToolCatalogEntry {
  name: string;
  description: string;
  source: "builtin" | "mcp" | "plugin" | "config";
  origin: ToolOrigin | null;
  provides_capabilities: string[];
  requires_confirmation: boolean;
  invocations_today: number;
  version: string;
  author: string;
}

/** `ConnectorStatus` — id/name/status/configured and nothing else (GAP-17). */
export interface Connector {
  id: string;
  name: string;
  status: string;
  configured: boolean;
}

export type ConnectorAction = "enable" | "disable" | "delete";

/**
 * Health only — metrics keyed by `skill_id`, with no name and no description:
 * `GET /v1/skills/health` is still the only skill route (GAP-18's skill half).
 */
export interface SkillHealthMetrics {
  skill_id: string;
  total_invocations: number;
  clean_success_rate: number;
  clean_success_rate_7d: number;
  repair_rate: number;
  repair_effectiveness: number;
  degraded_rate: number;
  avg_duration_ms: number;
  avg_cost_usd: number;
  avg_rounds: number;
  last_invoked_at: string | null;
  user_satisfaction_rate: number | null;
  feedback_count: number;
  feedback_coverage: number;
}

// ── Telemetry / health ──────────────────────────────────────────────────────

/**
 * Persisted event row. `task_id` is the run it happened inside — filled since
 * migration 037 for every arm that knows its run, `null` for an event that
 * belongs to none (and on rows written before the column existed).
 */
export interface EventLogRecord {
  id: number;
  timestamp: string;
  agent_id: string | null;
  task_id: string | null;
  event_type: string;
  detail?: unknown;
  result?: unknown;
}

/** `GET /v1/health` — unauthenticated, and exactly these four fields. */
export interface HealthResponse {
  status: string;
  version: string;
  pid: number;
  instance_id: string;
}

export interface DaemonProvidersResponse {
  web_search: {
    api_key_configured: boolean;
    api_key_hint: string;
    timeout_secs: number;
  };
}

// ── Shared helpers ──────────────────────────────────────────────────────────

/** `completed` is the daemon's terminal-success value; the design calls it `done`. */
export function toRunStatus(status: TaskStatusValue): RunStatus {
  return status === "completed" ? "done" : status;
}

export function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
