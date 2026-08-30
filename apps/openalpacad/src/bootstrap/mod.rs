//! Bootstrap helpers: config resolution, persona loading, and data migration.

mod config;
mod migration;
mod persona;

#[cfg(test)]
mod tests;

pub use config::{resolve_config_base_dir, seed_default_configs};
pub use migration::{
    is_same_file_path, migrate_preference_summaries, resolve_local_user_id, sweep_orphaned_tasks,
};
pub use persona::{
    bootstrap_bootstrap_document, bootstrap_identity_document, bootstrap_system_persona,
    bootstrap_user_document, load_bootstrap_document_from_file,
    load_identity_document_from_file, load_system_persona_from_soul_file,
    load_user_document_from_file,
};
