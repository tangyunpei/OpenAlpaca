//! Embedded database schema and subsequent migrations.
//!
//! A migration's file number is its schema version: `001_baseline.sql`
//! registers version 1, and the next migration is `002_<name>.sql`,
//! registering version 2. The unreleased development history was squashed
//! into that baseline, so a database from before it is refused rather than
//! upgraded (`database/mod.rs`).

/// A database migration.
pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Migrations in execution order. Future schema changes start at version 5:
/// add `005_<name>.sql` ending in `INSERT INTO schema_version(version) VALUES (5);`
/// and append its entry here.
///
/// Each file ends by registering its own version (`run_migrations` executes a
/// migration's SQL and nothing else). A fresh database runs all of them and
/// opens at the last one's version.
pub static MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "baseline",
        sql: include_str!("001_baseline.sql"),
    },
    Migration {
        version: 2,
        name: "working_state",
        sql: include_str!("002_working_state.sql"),
    },
    Migration {
        version: 3,
        name: "no_project_store_rows",
        sql: include_str!("003_no_project_store_rows.sql"),
    },
    Migration {
        version: 4,
        name: "project_files",
        sql: include_str!("004_project_files.sql"),
    },
];

/// Oldest supported populated schema; zero represents a fresh database. It
/// stays the baseline's version when later migrations are added: a fresh
/// database opens at `MIGRATIONS.last()`, not here.
pub const BASELINE_VERSION: i32 = MIGRATIONS[0].version;
