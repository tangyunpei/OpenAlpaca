//! Database migrations module
//!
//! Embeds SQL migration files and provides version tracking.

/// A database migration
pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// All migrations in order of execution
pub static MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "init",
        sql: include_str!("001_init.sql"),
    },
    Migration {
        version: 2,
        name: "memory_fts",
        sql: include_str!("002_memory_fts.sql"),
    },
    Migration {
        version: 3,
        name: "identity",
        sql: include_str!("003_identity.sql"),
    },
    Migration {
        version: 4,
        name: "config",
        sql: include_str!("004_config.sql"),
    },
    Migration {
        version: 5,
        name: "preference",
        sql: include_str!("005_preference.sql"),
    },
    Migration {
        version: 6,
        name: "tasks",
        sql: include_str!("006_tasks.sql"),
    },
    Migration {
        version: 7,
        name: "subagents",
        sql: include_str!("007_subagents.sql"),
    },
    Migration {
        version: 8,
        name: "llm_usage",
        sql: include_str!("008_llm_usage.sql"),
    },
    Migration {
        version: 9,
        name: "conversation_messages",
        sql: include_str!("009_conversation_messages.sql"),
    },
    Migration {
        version: 10,
        name: "discovered_models",
        sql: include_str!("010_discovered_models.sql"),
    },
    Migration {
        version: 11,
        name: "unified_conversations",
        sql: include_str!("011_unified_conversations.sql"),
    },
    Migration {
        version: 12,
        name: "assignment_output",
        sql: include_str!("012_assignment_output.sql"),
    },
    Migration {
        version: 13,
        name: "vec_search",
        sql: include_str!("013_vec_search.sql"),
    },
    Migration {
        version: 14,
        name: "conversation_summary",
        sql: include_str!("014_conversation_summary.sql"),
    },
    Migration {
        version: 15,
        name: "memory_v2",
        sql: include_str!("015_memory_v2.sql"),
    },
    Migration {
        version: 16,
        name: "task_state",
        sql: include_str!("016_task_state.sql"),
    },
    Migration {
        version: 17,
        name: "vec_768",
        sql: include_str!("017_vec_768.sql"),
    },
    Migration {
        version: 18,
        name: "memory_lifecycle",
        sql: include_str!("018_memory_lifecycle.sql"),
    },
    Migration {
        version: 19,
        name: "memory_fixes",
        sql: include_str!("019_memory_fixes.sql"),
    },
    Migration {
        version: 20,
        name: "agent_template_id",
        sql: include_str!("020_agent_template_id.sql"),
    },
    Migration {
        version: 21,
        name: "agent_template_id_not_null",
        sql: include_str!("021_agent_template_id_not_null.sql"),
    },
    Migration {
        version: 22,
        name: "orchestrator_latency",
        sql: include_str!("022_orchestrator_latency.sql"),
    },
    Migration {
        version: 23,
        name: "dispatch_decision",
        sql: include_str!("023_dispatch_decision.sql"),
    },
    Migration {
        version: 24,
        name: "dispatch_decision_request_id",
        sql: include_str!("024_dispatch_decision_request_id.sql"),
    },
    Migration {
        version: 25,
        name: "dispatch_decision_error_message",
        sql: include_str!("025_dispatch_decision_error_message.sql"),
    },
    Migration {
        version: 26,
        name: "memory_scope_dedup",
        sql: include_str!("026_memory_scope_dedup.sql"),
    },
    Migration {
        version: 27,
        name: "file_assets",
        sql: include_str!("027_file_assets.sql"),
    },
    Migration {
        version: 28,
        name: "message_attachments",
        sql: include_str!("028_message_attachments.sql"),
    },
    Migration {
        version: 29,
        name: "task_outcome",
        sql: include_str!("029_task_outcome.sql"),
    },
    Migration {
        version: 30,
        name: "skill_tool_execution_log",
        sql: include_str!("030_skill_tool_execution_log.sql"),
    },
    Migration {
        version: 31,
        name: "message_feedback",
        sql: include_str!("031_message_feedback.sql"),
    },
    Migration {
        version: 32,
        name: "context_compaction_log",
        sql: include_str!("032_context_compaction_log.sql"),
    },
    Migration {
        version: 33,
        name: "lane_followups",
        sql: include_str!("033_lane_followups.sql"),
    },
    Migration {
        version: 34,
        name: "drop_context_compaction_log",
        sql: include_str!("034_drop_context_compaction_log.sql"),
    },
];
