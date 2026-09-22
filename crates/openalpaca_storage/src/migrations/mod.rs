//! Embedded database schema and subsequent migrations.
//!
//! The unreleased migration history starts fresh at baseline version 1.
//! The database runner records each migration in `schema_migrations` atomically
//! with its SQL. Earlier development databases are not upgraded.

/// A database migration.
pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Migrations in execution order. Append future schema changes at version 2 onward.
/// SQL files contain schema/data changes only; the runner records their versions.
pub static MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "baseline",
    sql: include_str!("001_baseline.sql"),
}];
