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
];
