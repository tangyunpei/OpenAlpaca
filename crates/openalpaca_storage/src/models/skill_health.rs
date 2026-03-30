use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillHealthMetrics {
    pub skill_id: String,
    pub total_invocations: u64,
    pub clean_success_rate: f64,
    pub clean_success_rate_7d: f64,
    pub repair_rate: f64,
    pub repair_effectiveness: f64,
    pub degraded_rate: f64,
    pub avg_duration_ms: f64,
    pub avg_cost_usd: f64,
    pub avg_rounds: f64,
    pub last_invoked_at: Option<DateTime<Utc>>,
    pub user_satisfaction_rate: Option<f64>,
    pub feedback_count: u64,
    pub feedback_coverage: f64,
}
