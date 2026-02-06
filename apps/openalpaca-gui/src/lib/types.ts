/**
 * Shared TypeScript type definitions matching backend REST response shapes.
 */

// ── Task types ──────────────────────────────────────────────────────

export type TaskStatusValue = "queued" | "running" | "completed" | "failed" | "cancelled" | "paused";

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
