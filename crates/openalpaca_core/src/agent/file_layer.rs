//! The file layer's rescan: `<config>/agents/` read whole into
//! `AgentRegistry`'s file layer (subagent design §2.4 L-3,
//! `tasks/subagent-brief-design.md`).
//!
//! Every change to the directory — the start, the watcher, the template
//! routes — rereads every top-level `.md` and replaces the layer in one call,
//! so a file deleted, renamed or given a new `id:` leaves the catalog and
//! nothing keeps a copy the directory no longer holds. The legacy `.toml`
//! agents are not templates: the daemon reads them at start, as before.

use super::registry::{AgentRegistry, FileLayer, NotUsedReason, same_definition};
use super::template::{AgentTemplate, lenient_template_id, parse_agent_markdown};
use crate::tools::registry::validate_annotation_capability;
use openalpaca_storage::Database;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Who asks for the rescan, which decides what an unknown `annotation:`
/// capability does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadMode {
    /// The daemon's start: an unknown `annotation:` capability is an error,
    /// and the daemon does not start (extension design §10 case 2).
    Start,
    /// The watcher and the template routes: the file's id is withheld
    /// instead, with a `WARN` — a running daemon's catalog stays what its
    /// next start accepts.
    Reload,
}

/// One top-level `.md` of the directory, read.
struct Read {
    path: PathBuf,
    parsed: Result<AgentTemplate, Broken>,
}

/// A file that does not parse, and the id it names leniently.
struct Broken {
    id: Option<String>,
    error: String,
}

/// Reread `agents_dir` whole and replace `registry`'s file layer with what
/// it holds; then write the `agent` row of each file template whose content
/// changed and sync the idle instances (L-1a).
///
/// Files are read in file-name order. Of two files claiming one id the first
/// holds it and the other is recorded not used, with a `WARN` naming both.
/// A file that does not parse never takes an id from a file that parses,
/// whichever sorts first: it is recorded not used, with a `WARN`. Otherwise
/// it holds the id it names (its `id:` line, else its file stem) as withheld
/// (L-4) when a layer of the registry held that id before the rescan, and
/// is skipped with a `WARN`, as before, when none did. A directory that
/// cannot be read leaves the layer as it was; one that does not exist
/// empties it.
///
/// One rescan runs at a time on a registry: a route's and the watcher's,
/// each reading the directory then replacing the layer, would otherwise
/// interleave, and the one that read first could replace the layer last.
///
/// Returns an error only in `LoadMode::Start`, for an unknown `annotation:`
/// capability, worded as the start always worded it.
pub fn rescan_file_layer(
    agents_dir: &Path,
    mode: LoadMode,
    registry: &AgentRegistry,
    known_capabilities: &[String],
    db: &Database,
) -> Result<(), String> {
    let _one_at_a_time = registry.lock_rescan();
    let files = match read_template_files(agents_dir) {
        Ok(files) => files,
        Err(e) => {
            warn!(
                "Agent templates: cannot read {}: {e}; the catalog is left as it was",
                agents_dir.display()
            );
            return Ok(());
        }
    };

    // The start refuses a file naming an unknown capability wherever it
    // sorts, a duplicate included, as it always has.
    if mode == LoadMode::Start {
        for file in &files {
            if let Ok(template) = &file.parsed {
                check_capabilities(template, known_capabilities)
                    .map_err(|e| format!("{}: {e}", file.path.display()))?;
            }
        }
    }

    // The files that parse first, then the broken ones: a broken file never
    // takes an id from one that parses, whichever sorts first.
    let mut layer = FileLayer::new();
    let mut loaded = Vec::new();
    let mut broken_files = Vec::new();
    for file in &files {
        let path = &file.path;
        match &file.parsed {
            Ok(template) => {
                let id = &template.frontmatter.id;
                if layer.holds(id) {
                    note_duplicate(&mut layer, id, path);
                    continue;
                }
                if let Err(e) = check_capabilities(template, known_capabilities) {
                    // Reload only: the start returned above.
                    let reason = format!("names an unknown capability ({e})");
                    withhold(&mut layer, id, path, reason);
                    continue;
                }
                layer.insert(template.clone(), Some(path.clone()));
                loaded.push((id.clone(), path));
                if mode == LoadMode::Start {
                    info!(
                        "Loaded agent template: {} (singleton={})",
                        path.display(),
                        template.frontmatter.singleton
                    );
                }
            }
            Err(broken) => broken_files.push((path, broken)),
        }
    }
    for (path, broken) in broken_files {
        let named = broken.id.clone().or_else(|| file_stem(path));
        match named {
            Some(id) if layer.holds(&id) => note_broken(&mut layer, &id, path, &broken.error),
            Some(id) if registry.provenance(&id).is_some() => {
                let reason = format!("does not parse ({})", broken.error);
                withhold(&mut layer, &id, path, reason);
            }
            _ => warn!(
                "Failed to parse agent template {}: {}",
                path.display(),
                broken.error
            ),
        }
    }

    let before: HashMap<&str, AgentTemplate> = loaded
        .iter()
        .filter_map(|(id, _)| registry.get_template(id).map(|t| (id.as_str(), t)))
        .collect();
    registry.replace_file_layer(layer);
    for (id, path) in &loaded {
        let Some(template) = registry.get_template(id) else {
            continue;
        };
        let changed = before
            .get(id.as_str())
            .is_none_or(|old| !same_definition(old, &template));
        if changed {
            persist_template_to_db(db, &template, id);
            if mode == LoadMode::Reload {
                info!("Agent template hot-reloaded: {}", path.display());
            }
        }
    }
    registry.sync_template_instances();
    Ok(())
}

/// Every top-level `.md` in `agents_dir`, read and parsed, in file-name
/// order (byte order). A directory that does not exist holds none.
fn read_template_files(agents_dir: &Path) -> std::io::Result<Vec<Read>> {
    let entries = match std::fs::read_dir(agents_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("md") && path.is_file())
        .collect();
    paths.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(paths
        .into_iter()
        .map(|path| {
            let parsed = match std::fs::read(&path) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => parse_agent_markdown(&text).map_err(|e| Broken {
                        id: lenient_template_id(&text),
                        error: e.to_string(),
                    }),
                    Err(e) => Err(Broken {
                        id: lenient_template_id(&String::from_utf8_lossy(e.as_bytes())),
                        error: "the file is not UTF-8".to_string(),
                    }),
                },
                Err(e) => Err(Broken {
                    id: None,
                    error: format!("cannot be read: {e}"),
                }),
            };
            Read { path, parsed }
        })
        .collect())
}

fn file_stem(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .map(str::to_string)
}

/// The start's check of a template's `annotation:` names, worded as it
/// always was.
fn check_capabilities(template: &AgentTemplate, known: &[String]) -> Result<(), String> {
    for cap in &template.frontmatter.capabilities {
        validate_annotation_capability(cap, known).map_err(|e| format!("capabilities: {e}"))?;
    }
    for cap in &template.frontmatter.denied_capabilities {
        validate_annotation_capability(cap, known)
            .map_err(|e| format!("denied_capabilities: {e}"))?;
    }
    Ok(())
}

fn withhold(layer: &mut FileLayer, id: &str, path: &Path, reason: String) {
    warn!(
        "Agent template {} {reason}; template {id} is withheld until it is fixed or removed",
        path.display()
    );
    layer.withhold(id.to_string(), path.to_path_buf(), reason);
}

/// A file that does not parse naming an id another file holds: it is not
/// used, and the `WARN` says which file holds the id.
fn note_broken(layer: &mut FileLayer, id: &str, path: &Path, error: &str) {
    match layer.path_of(id) {
        Some(holder) => warn!(
            "Failed to parse agent template {}: {error}; {} holds the id {id}",
            path.display(),
            holder.display()
        ),
        None => warn!(
            "Failed to parse agent template {}: {error}; the id {id} is held",
            path.display()
        ),
    }
    layer.note_not_used(id, path.to_path_buf(), NotUsedReason::DoesNotParse);
}

fn note_duplicate(layer: &mut FileLayer, id: &str, path: &Path) {
    if let Some(first) = layer.path_of(id) {
        warn!(
            "Agent templates {} and {} both claim the id {id}; {} is used (the first by file name)",
            first.display(),
            path.display(),
            first.display()
        );
    }
    layer.note_not_used(id, path.to_path_buf(), NotUsedReason::Duplicate);
}

/// Write a template's `agent` row — the row `GET /v1/agents` lists and the
/// run history references (design §2.11) — and its metrics row if missing.
fn persist_template_to_db(db: &Database, template: &AgentTemplate, template_id: &str) {
    let repo = openalpaca_storage::SubAgentRepository::new(db);
    let persona = super::template::extract_persona(template);
    let fm = &template.frontmatter;
    let skills: Vec<super::Capability> = fm
        .capabilities
        .iter()
        .map(|s| super::Capability {
            name: s.clone(),
            category: "assigned".to_string(),
            proficiency: 1.0,
        })
        .collect();
    let preset = super::AgentPreset {
        persona: persona.clone(),
        temperature: fm.temperature,
        verbosity: fm.verbosity.clone(),
    };
    let constraints = super::AgentConstraints {
        max_tool_calls: fm.max_tool_calls,
        timeout_seconds: fm.timeout_seconds,
        max_cost_per_task: fm.max_cost_per_task,
        require_confirmation_for: fm.require_confirmation_for.clone(),
        allowed_capabilities: fm.capabilities.clone(),
        denied_capabilities: fm.denied_capabilities.clone(),
        ..Default::default()
    };
    let llm_config = super::AgentLlmConfig {
        model: fm.model.clone(),
        fallback_models: fm.fallback_models.clone(),
        ..Default::default()
    };
    let now = chrono::Utc::now();
    let storage_config = openalpaca_storage::SubAgentConfig {
        id: template_id.to_string(),
        template_id: template_id.to_string(),
        name: fm.name.clone(),
        description: Some(fm.description.clone()),
        icon: fm.icon.clone(),
        status: "idle".to_string(),
        current_task_id: None,
        skills_json: serde_json::to_string(&skills).unwrap_or_else(|_| "[]".to_string()),
        preset_json: serde_json::to_string(&preset).unwrap_or_else(|_| "{}".to_string()),
        constraints_json: Some(
            serde_json::to_string(&constraints).unwrap_or_else(|_| "{}".to_string()),
        ),
        llm_config_json: Some(
            serde_json::to_string(&llm_config).unwrap_or_else(|_| "{}".to_string()),
        ),
        persona: Some(persona),
        created_at: now,
        updated_at: Some(now),
    };
    let _ = repo.upsert(&storage_config);

    // Initialize metrics row if not exists
    if let Ok(None) = repo.get_metrics(template_id) {
        let _ = repo.upsert_metrics(&openalpaca_storage::AgentMetrics::new_empty(template_id));
    }
}

#[cfg(test)]
mod tests;
