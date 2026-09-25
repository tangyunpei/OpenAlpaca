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

/// Migrations in execution order. Future schema changes start at version 46.
///
/// Each file ends by registering its own version (`run_migrations` executes a
/// migration's SQL and nothing else). A fresh database runs all of them and
/// opens at the last one's version.
pub static MIGRATIONS: &[Migration] = &[
    Migration {
        version: 43,
        name: "baseline",
        sql: include_str!("001_baseline.sql"),
    },
    // Learning v2 (tasks/memory-redesign/09 §1.3): `request_id` and
    // `workspace_root` on `conversation_messages`, and the per-session
    // `learning_cursor`, seeded at each existing session's newest message.
    Migration {
        version: 44,
        name: "learning",
        sql: include_str!("044_learning.sql"),
    },
    // Conversation summaries (tasks/memory-redesign/09 §13.3):
    // `learning_cursor.episode_through`, the last message a session's
    // `episode` notes have summarised, seeded at each existing session's
    // newest message.
    Migration {
        version: 45,
        name: "episodes",
        sql: include_str!("045_episodes.sql"),
    },
];

/// Oldest supported populated schema; zero represents a fresh database. It
/// stays the baseline's version when later migrations are added: a fresh
/// database opens at `MIGRATIONS.last()`, not here.
pub const BASELINE_VERSION: i32 = MIGRATIONS[0].version;
