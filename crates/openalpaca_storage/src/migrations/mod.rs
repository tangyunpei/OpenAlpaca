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
];
