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

/** Summary row injected into `GET /v1/tasks` list items only. */
export interface TaskAssignedAgent {
  agent_id: string;
  role: string;
  status: string;
  runtime_seconds: number | null;
  completed_at: string | null;
}

/** Free-form artifact reference parsed out of `task.outcome_json`. Schema-less by design — see GAP-04. */
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
  /** List route only — the detail route returns `assignments` instead. */
  assigned_agents?: TaskAssignedAgent[];
  outcome?: ParsedOutcome;
}

export type AssignmentStatusValue =
  "pending" | "running" | "completed" | "failed";

/** One agent run on a task; served under the legacy `assignments` key. */
export interface TaskAgentAssignment {
  id: string;
  task_id: string;
  agent_id: string;
  role: string;
  status: AssignmentStatusValue;
  runtime_seconds: number | null;
  completed_at: string;
}

/**
 * `GET /v1/tasks/{id}`. Deliberately a different shape from a list row —
 * API_MAP §5 warns the two disagree; do not conflate them.
 */
export interface TaskDetailResponse {
  task: Task;
  assignments: TaskAgentAssignment[] | null;
  outcome?: ParsedOutcome;
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

/** The only verbs `apply_task_action` accepts. `rerun`/`start` are GAP-06. */
export type TaskAction = "cancel" | "pause" | "resume";

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

/** `ConversationMessage`. Has no `task_id` and no artifact refs — GAP-23. */
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

/** `ApprovalScope` (`security/confirmation.rs`). Dropped by the HTTP route today — GAP-01. */
export type ApprovalScope = "these_args" | "entire_tool";

export interface ConfirmationRequestBody {
  approved: boolean;
  /** Sent optimistically; the daemon currently discards it (GAP-01). */
  approval_scope?: ApprovalScope;
}

export type FeedbackValue = "positive" | "negative";

export interface FeedbackResponse {
  message_id: number;
  feedback: FeedbackValue;
  comment: string | null;
}

// ── Conversations ───────────────────────────────────────────────────────────

export interface Conversation {
  id: string;
  lane_key: string;
  source: string;
  title: string;
  message_count: number;
  last_message_at: string | null;
  created_at: string;
  updated_at: string;
  summary: string;
  summary_version: number;
  last_summarized_message_id: number;
  summary_updated_at?: string | null;
}

export interface ConversationsResponse {
  conversations: Conversation[];
}

export interface ConversationMessagesResponse {
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

/** `health` is hardcoded `"healthy"`; `total_tokens` is lifetime, not today (GAP-08). */
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

/** `daily_cost_usd` is hardcoded `0.0` server-side — GAP-08.2. */
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
  planner_ms: number;
  dispatch_ms: number;
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
  mean_planner_ms: number;
  mean_dispatch_ms: number;
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
  planner_requested_mode: string | null;
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

// ── Plugins / connectors / skills ───────────────────────────────────────────

export interface PluginInfo {
  name: string;
  version: string;
  status: string;
  tools: string[];
  connector: string | null;
  provider: string | null;
  models: string[];
}

export type PluginAction = "approve" | "deny" | "enable" | "disable";

/** `ConnectorStatus` — id/name/status/configured and nothing else (GAP-17). */
export interface Connector {
  id: string;
  name: string;
  status: string;
  configured: boolean;
}

export type ConnectorAction = "enable" | "disable" | "delete";

/** Health only — no name, description, `asks` badge, or enabled flag (GAP-18). */
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

/** Persisted event row. Has no `task_id` column — GAP-10. */
export interface EventLogRecord {
  id: number;
  timestamp: string;
  agent_id: string | null;
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
