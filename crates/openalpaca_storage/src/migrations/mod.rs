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
];
