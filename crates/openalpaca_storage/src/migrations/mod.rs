//! Embedded database schema and subsequent migrations.
//!
//! The unreleased migration history is squashed into one baseline. The memory
//! redesign re-squashed it as version 43 without the SQL memory tables, so a
//! database at 42 — the last schema with them — is refused like every older
//! development schema rather than upgraded (`database/mod.rs`, plan §4).

/// A database migration.
pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Migrations in execution order. Future schema changes start at version 44.
pub static MIGRATIONS: &[Migration] = &[Migration {
    version: 43,
    name: "baseline",
    sql: include_str!("001_baseline.sql"),
}];

/// Oldest supported populated schema; zero represents a fresh database.
pub const BASELINE_VERSION: i32 = MIGRATIONS[0].version;
