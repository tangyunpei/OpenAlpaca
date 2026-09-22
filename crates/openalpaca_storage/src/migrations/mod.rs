//! Embedded database schema and subsequent migrations.
//!
//! The unreleased migration history is squashed into one baseline. Its version
//! remains 42 so databases already at that schema can reopen without changes.
//! Older development databases are not upgraded by the baseline.

/// A database migration.
pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Migrations in execution order. Future schema changes start at version 43.
pub static MIGRATIONS: &[Migration] = &[Migration {
    version: 42,
    name: "baseline",
    sql: include_str!("001_baseline.sql"),
}];

/// Oldest supported populated schema; zero represents a fresh database.
pub const BASELINE_VERSION: i32 = MIGRATIONS[0].version;
