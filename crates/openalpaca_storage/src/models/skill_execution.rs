use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExecutionEntry {
    pub id: Option<i64>,
    pub request_id: String,
    pub skill_id: String,
    pub agent_id: String,
    pub status: String,
    pub finish_reason: Option<String>,
    pub error_message: Option<String>,
    pub validation_failures: Option<String>,
    pub duration_ms: i64,
    pub rounds_used: Option<i32>,
    pub tool_calls_made: Option<i32>,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub cost_usd: f64,
    pub model_used: Option<String>,
    pub query_preview: Option<String>,
    pub route_score: Option<f64>,
    pub was_auto_selected: bool,
    pub repair_attempted: bool,
    pub repair_succeeded: bool,
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionEntry {
    pub id: Option<i64>,
    pub request_id: Option<String>,
    pub agent_id: String,
    pub tool_name: String,
    pub success: bool,
    pub duration_ms: i64,
    pub error_message: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
}
