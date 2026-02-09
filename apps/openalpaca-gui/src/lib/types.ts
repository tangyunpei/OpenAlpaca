/**
 * Shared TypeScript type definitions matching backend REST response shapes.
 */

// ── Task types ──────────────────────────────────────────────────────

export type TaskStatusValue = "queued" | "running" | "completed" | "failed" | "cancelled" | "paused";

export interface TaskAssignedAgent {
  agent_id: string;
  role: string;
  status: string;
}

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
  assigned_agents?: TaskAssignedAgent[];
}

export type AssignmentStatusValue = "pending" | "running" | "completed" | "failed";

export interface TaskAgentAssignment {
  id: string;
  task_id: string;
  agent_id: string;
  role: string;
  status: AssignmentStatusValue;
  step_order: number | null;
  started_at: string | null;
  completed_at: string | null;
  result_output: string | null;
}

export interface TaskDetailResponse {
  task: Task;
  assignments: TaskAgentAssignment[] | null;
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

// ── Skill type ──────────────────────────────────────────────────────

export interface Skill {
  name: string;
  category: string;
  proficiency: number;
}

// ── Agent types ─────────────────────────────────────────────────────

export type AgentStatusValue = "idle" | "busy" | "waiting" | "offline" | "error";

export interface Agent {
  id: string;
  name: string;
  description: string | null;
  icon: string | null;
  status: string;
  current_task_id: string | null;
  skills_json: string;
  preset_json: string;
  constraints_json: string | null;
  llm_config_json: string | null;
  persona: string | null;
  created_at: string;
  updated_at: string | null;
}

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

// ── Settings types ──────────────────────────────────────────────────

export type KeyPriorityValue = "primary" | "fallback";
export type KeySourceValue = "api_console" | "claude_code" | "claude_max_pro" | "codex" | "environment" | "other";
export type KeyHealthValue = "healthy" | "rate_limited" | "error" | "unknown";

export interface LlmSettingsResponse {
  orchestrator: OrchestratorInfo;
  providers: Record<string, ProviderInfo>;
}

export interface OrchestratorInfo {
  model: string;
  fallback_models: string[];
}

export interface ProviderInfo {
  enabled: boolean;
  key_selection_strategy: string;
  keys: KeyInfo[];
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

export interface ExternalUsage {
  period: string;
  cost_usd: number;
  token_count: number;
  rate_limit_remaining: number | null;
  fetched_at: string;
  approximate: boolean;
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

export interface ProviderUsageSummary {
  provider: string;
  total_cost_usd: number;
  total_tokens: number;
  total_requests: number;
  health: string;
  external_usage: ExternalUsage | null;
}

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

export interface KeyStatusMap {
  [provider: string]: Array<{
    id: string;
    health: KeyHealthValue;
    consecutive_rate_limits: number;
    is_available: boolean;
  }>;
}

// ── Model types ────────────────────────────────────────────────────

export interface ModelEntry {
  id: string;
  provider: string;
  context_window: number;
  input_price_per_million: number;
  output_price_per_million: number;
}

// ── Agent Config types ─────────────────────────────────────────────

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

export interface UpdateAgentConfigRequest {
  config: AgentConfigFile;
  config_version: number;
}

export interface CreateAgentRequest {
  config: AgentConfigFile;
}

export interface CreateAgentFromTomlRequest {
  toml_content: string;
}

// ── Orchestrator Config types ──────────────────────────────────────

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

// ── Chat types ─────────────────────────────────────────────────────

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
}

export interface ChatSendRequest {
  content: string;
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

export interface ChatStreamDoneData {
  content: string;
  model: string;
  tokens_in: number;
  tokens_out: number;
  duration_ms: number;
}

// ── Conversation types ────────────────────────────────────────────

export interface Conversation {
  id: string;
  lane_key: string;
  source: string;
  title: string;
  message_count: number;
  last_message_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface ConversationsResponse {
  conversations: Conversation[];
}

export interface ConversationMessagesResponse {
  messages: ChatMessage[];
  total: number;
}

// ── LLM Usage types ──────────────────────────────────────────────

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
