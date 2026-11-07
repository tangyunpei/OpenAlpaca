//! The file layer's rescan: `<config>/agents/` read whole into
//! `AgentRegistry`'s file layer (subagent design §2.4 L-3,
//! `tasks/subagent-brief-design.md`).
//!
//! Every change to the directory — the start, the watcher, the template
//! routes — rereads every top-level `.md` and replaces the layer in one call,
//! so a file deleted, renamed or given a new `id:` leaves the catalog and
//! nothing keeps a copy the directory no longer holds. The file layer sits
//! over the bundled one (`crate::bundled`): a file overrides the bundled
//! template of its `id`, and an unedited copy of a shipped version is set
//! aside (§2.2), so a file deleted brings the bundled template back. A file
//! saying `disabled: true` switches the id it names off (§2.4 L-4a). The
//! legacy `.toml` agents are not templates: the daemon reads them at start,
//! as before.

use super::registry::{AgentRegistry, FileLayer, NotUsedReason, TemplateLayer, same_definition};
use super::template::{AgentTemplate, lenient_disabled, lenient_template_id, parse_agent_markdown};
use crate::bundled::{ShippedCopy, ShippedVersion, classify_agent_file};
use crate::tools::ToolRegistry;
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
    /// Set when the bytes are a version OpenAlpaca shipped (V-1, V-4).
    copy: Option<ShippedCopy>,
    content: Content,
}

/// What a file holds.
enum Content {
    Template(Box<AgentTemplate>),
    /// The frontmatter says `disabled: true` (L-4a): the file switches off
    /// the id its `id:` line names, else its file stem, and nothing else in
    /// it is read.
    Disabled {
        id: Option<String>,
    },
    Broken(Broken),
}

impl Read {
    /// The id the file names: its template's, else its `id:` line read
    /// leniently, else its file stem.
    fn id(&self) -> Option<String> {
        match &self.content {
            Content::Template(template) => Some(template.frontmatter.id.clone()),
            Content::Disabled { id } | Content::Broken(Broken { id, .. }) => {
                id.clone().or_else(|| file_stem(&self.path))
            }
        }
    }
}

/// A file that does not parse, and the id it names leniently.
struct Broken {
    id: Option<String>,
    error: String,
}

/// Reread `agents_dir` whole and replace `registry`'s file layer with what
/// it holds; then write the `agent` row of each template whose resolved
/// definition changed — a bundled one's included (§2.11) — and sync the
/// idle instances (L-1a).
///
/// A file byte-identical to a version OpenAlpaca shipped is set aside when
/// the id it names is a bundled template's (V-1, V-4, V-6): not read, not
/// touched, recorded on that id's row, and the bundled template runs.
/// The other files are read in file-name order. Of two files claiming one
/// id the first holds it and the other is recorded not used, with a `WARN`
/// naming both. A file that does not parse never takes an id from a file
/// that parses, whichever sorts first: it is recorded not used, with a
/// `WARN`. Otherwise it holds the id it names (its `id:` line, else its file
/// stem) as withheld (L-4) when a layer of the registry held that id before
/// the rescan — the bundled one included, so a broken override never gives
/// way to the bundled template (U2) — and is skipped with a `WARN`, as
/// before, when none did. A file whose frontmatter says `disabled: true`
/// switches off the id it names (L-4a), taking its place in the file-name
/// order as a template does; its `agent` row reads `disabled` while it
/// does, and `idle` again once it does not and a template runs under the
/// id. A template whose
/// `require_confirmation_for` names no tool is warned about (W-1). A
/// directory that cannot be read leaves the layer as it was; one that does
/// not exist empties it.
///
/// `tools` is the tool registry: the `annotation:` names it knows, and the
/// tools W-1 checks a confirmation list against.
///
/// One rescan runs at a time on a registry: a route's and the watcher's,
/// each reading the directory then replacing the layer, would otherwise
/// interleave, and the one that read first could replace the layer last.
///
/// Returns an error only in `LoadMode::Start`, for an unknown `annotation:`
/// capability, worded as the start always worded it. At start it logs §1.3's
/// summary, then one line per override, then one per file that switches an
/// id off; a reload logs the last when a file newly switches an id off.
pub fn rescan_file_layer(
    agents_dir: &Path,
    mode: LoadMode,
    registry: &AgentRegistry,
    tools: &ToolRegistry,
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
    let known_capabilities = tools.known_virtual_capabilities();

    // The unedited copies of shipped versions that would hide a bundled
    // template are set aside first: they are not read, so nothing below —
    // the start's check included — sees them.
    let mut layer = FileLayer::new();
    let mut set_aside = Vec::new();
    let mut files_read = Vec::new();
    for file in files {
        if let Some((id, copy)) = set_aside_as(&file, |id| registry.is_bundled(id)) {
            layer.set_aside(&id, file.path.clone(), copy);
            set_aside.push((file.path, copy));
            continue;
        }
        files_read.push(file);
    }

    // The start refuses a file naming an unknown capability wherever it
    // sorts, a duplicate included, as it always has. A file that switches
    // an id off is not read past its `disabled:` and `id:` lines.
    if mode == LoadMode::Start {
        for file in &files_read {
            if let Content::Template(template) = &file.content {
                check_capabilities(template, &known_capabilities)
                    .map_err(|e| format!("{}: {e}", file.path.display()))?;
            }
        }
    }

    // The files that parse or switch an id off first, then the broken ones:
    // a broken file never takes an id from either, whichever sorts first.
    let mut loaded = Vec::new();
    let mut switched_off = Vec::new();
    let mut broken_files = Vec::new();
    for file in &files_read {
        let path = &file.path;
        match &file.content {
            Content::Template(template) => {
                let id = &template.frontmatter.id;
                if layer.holds(id) {
                    note_duplicate(&mut layer, id, path);
                    continue;
                }
                if let Err(e) = check_capabilities(template, &known_capabilities) {
                    // Reload only: the start returned above.
                    let reason = format!("names an unknown capability ({e})");
                    withhold(&mut layer, id, path, reason);
                    continue;
                }
                layer.insert(AgentTemplate::clone(template), Some(path.clone()));
                for warning in confirmation_warnings(template, path, tools) {
                    warn!("{warning}");
                    layer.warn(id, warning);
                }
                loaded.push((id.clone(), path.clone()));
                if mode == LoadMode::Start {
                    info!(
                        "Loaded agent template: {} (singleton={})",
                        path.display(),
                        template.frontmatter.singleton
                    );
                }
            }
            Content::Disabled { .. } => {
                let Some(id) = file.id() else {
                    warn!(
                        "Agent template {} says disabled: true but names no id",
                        path.display()
                    );
                    continue;
                };
                if layer.holds(&id) {
                    note_duplicate(&mut layer, &id, path);
                    continue;
                }
                layer.disable(id.clone(), path.clone());
                switched_off.push((id, path.clone()));
            }
            Content::Broken(broken) => broken_files.push((path, broken)),
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

    let before = resolved_by_layer(registry);
    let disabled_before = registry.disabled_ids();
    registry.replace_file_layer(layer);
    let after = resolved_by_layer(registry);
    for (id, (layer, template)) in &after {
        let changed = mode == LoadMode::Start
            || before
                .get(id)
                .is_none_or(|(was, old)| was != layer || !same_definition(old, template));
        if !changed {
            continue;
        }
        persist_template_to_db(db, template, id);
        if mode == LoadMode::Reload {
            match (layer, registry.provenance(id).and_then(|p| p.path)) {
                (TemplateLayer::File, Some(path)) => {
                    info!("Agent template hot-reloaded: {}", path.display())
                }
                (TemplateLayer::Bundled, _) => {
                    info!("Agent template {id}: the bundled template runs again")
                }
                _ => {}
            }
        }
    }
    // L-4a: a switched-off id's row is kept, reading `disabled`; one no
    // longer switched off reads `idle` again when a template runs under it
    // (a file's or the bundle's was set back by its row's write above; a
    // plugin's is set back here). Under which nothing runs, the row stays
    // as it reads, as a row a daemon that was down left `disabled` does.
    let repo = openalpaca_storage::SubAgentRepository::new(db);
    for (id, path) in &switched_off {
        let _ = repo.update_status(id, "disabled", None);
        if mode == LoadMode::Reload && disabled_before.binary_search(id).is_err() {
            info!("{}", switch_off_line(path, id, registry.is_bundled(id)));
        }
    }
    for id in &disabled_before {
        if !switched_off.iter().any(|(off, _)| off == id)
            && registry.get_template(id).is_some()
            && let Ok(Some(row)) = repo.get(id)
            && row.status == "disabled"
        {
            let _ = repo.update_status(id, "idle", None);
        }
    }
    if mode == LoadMode::Start {
        let bundled = registry.bundled_count();
        let overriding: Vec<(&str, &Path)> = loaded
            .iter()
            .filter(|(id, _)| registry.is_bundled(id))
            .map(|(id, path)| (id.as_str(), path.as_path()))
            .collect();
        info!(
            "{}",
            start_summary(bundled, agents_dir, loaded.len(), &overriding, &set_aside)
        );
        for (id, path) in &overriding {
            info!(
                "Agent template {} overrides the bundled template {id}",
                path.display()
            );
        }
        for (id, path) in &switched_off {
            info!("{}", switch_off_line(path, id, registry.is_bundled(id)));
        }
    }
    registry.sync_template_instances();
    Ok(())
}

/// The resolved templates that are a file's or the bundle's — the ones
/// with an `agent` row and an idle instance — with their layer.
fn resolved_by_layer(registry: &AgentRegistry) -> HashMap<String, (TemplateLayer, AgentTemplate)> {
    registry
        .list_templates()
        .into_iter()
        .filter_map(|template| {
            let id = template.frontmatter.id.clone();
            let layer = registry.provenance(&id)?.layer;
            (layer != TemplateLayer::Plugin).then_some((id, (layer, template)))
        })
        .collect()
}

/// §1.3's start line: how many templates are bundled, how many files of
/// `dir` are read and which of them override a bundled template, and which
/// unedited copies are set aside.
pub(crate) fn start_summary(
    bundled: usize,
    dir: &Path,
    read: usize,
    overriding: &[(&str, &Path)],
    set_aside: &[(PathBuf, ShippedCopy)],
) -> String {
    let mut ids: Vec<&str> = overriding.iter().map(|(id, _)| *id).collect();
    ids.sort();
    let overrides = match ids.len() {
        0 => String::new(),
        k if k == read && k == 1 => format!(", overriding a bundled template ({})", ids.join(", ")),
        k if k == read => format!(", each overriding a bundled template ({})", ids.join(", ")),
        k => format!(
            ", {k} of them overriding a bundled template ({})",
            ids.join(", ")
        ),
    };
    let mut earlier: Vec<String> = set_aside
        .iter()
        .filter(|(_, copy)| copy.version == ShippedVersion::Earlier)
        .map(|(path, _)| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    earlier.sort();
    let copies = match set_aside.len() {
        0 => "no unedited copy of a shipped version set aside".to_string(),
        1 => "1 unedited copy of a shipped version set aside, not read".to_string(),
        m => format!("{m} unedited copies of shipped versions set aside, not read"),
    };
    let earlier = match (set_aside.len(), earlier.len()) {
        (_, 0) => String::new(),
        (1, _) => format!(" (an earlier version: {})", earlier.join(", ")),
        (_, 1) => format!(" (1 of them an earlier version: {})", earlier.join(", ")),
        (_, e) => format!(" ({e} of them earlier versions: {})", earlier.join(", ")),
    };
    format!(
        "Agent templates: {bundled} bundled; {read} from {}{overrides}; {copies}{earlier}",
        dir.display()
    )
}

/// §1.3's line for a file that switches an id off (L-4a), `INFO`: deliberate,
/// like an override.
fn switch_off_line(path: &Path, id: &str, bundled: bool) -> String {
    let which = if bundled {
        "the bundled template"
    } else {
        "the template"
    };
    format!(
        "Agent template {} switches off {which} {id} (disabled: true)",
        path.display()
    )
}

/// W-1: a confirmation list naming an entry that is no registered tool, no
/// plugin's tool and no capability the tool registry resolves. Such a list
/// still replaces the default prompts (`effective_confirmation_set`), so the
/// tools that would ask by default — this template's tools marked
/// destructive — run without asking unless the list names them. Nothing is
/// refused or rewritten.
fn confirmation_warnings(
    template: &AgentTemplate,
    path: &Path,
    tools: &ToolRegistry,
) -> Vec<String> {
    let list = &template.frontmatter.require_confirmation_for;
    let registered = tools.registered_tool_names();
    let known = tools.known_virtual_capabilities();
    // A plugin's tool (`<plugin>::<tool>`) is not checked: plugins register
    // their tools after the templates load at start, and add or drop them as
    // they are enabled or disabled, so its absence here says nothing.
    let unknown: Vec<&String> = list
        .iter()
        .filter(|entry| {
            !entry.contains("::")
                && !registered.contains(entry)
                && !known.contains(entry)
                && tools.capability_index_entry(entry).is_empty()
        })
        .collect();
    if unknown.is_empty() {
        return Vec::new();
    }

    // The tools that would ask by default, in the order the template grants
    // them; `file_edit` comes with `file_write` (R42), and a list naming
    // `file_write` asks for it too (R-T35).
    let destructive = |name: &str| {
        tools.iter_registered_tools().any(|(tool, reg)| {
            tool == name && reg.annotations.as_ref().and_then(|a| a.destructive_hint) == Some(true)
        })
    };
    let names = |tool: &str| list.iter().any(|entry| entry == tool);
    let mut silent: Vec<&str> = Vec::new();
    for capability in &template.frontmatter.capabilities {
        let capability = capability.as_str();
        let granted: Vec<&str> = if capability == "file_write" {
            vec!["file_write", "file_edit"]
        } else {
            vec![capability]
        };
        for tool in &granted {
            let asked = names(tool) || (*tool == "file_edit" && names("file_write"));
            if !asked && destructive(tool) && !silent.contains(tool) {
                silent.push(tool);
            }
        }
    }
    let tail = match silent.as_slice() {
        [] => String::new(),
        [one] => format!(", so this template's {one} runs without asking"),
        [rest @ .., last] => format!(
            ", so this template's {} and {last} run without asking",
            rest.join(", ")
        ),
    };
    unknown
        .into_iter()
        .map(|entry| {
            format!(
                "{}: require_confirmation_for names \"{entry}\", which is no tool; this list \
                 replaces the default prompts{tail}",
                path.display()
            )
        })
        .collect()
}

/// The id `file` would be set aside under, and what copy it is: a file
/// byte-identical to a version OpenAlpaca shipped, naming an id
/// `is_bundled` holds (V-1, V-4, V-6).
fn set_aside_as(file: &Read, is_bundled: impl Fn(&str) -> bool) -> Option<(String, ShippedCopy)> {
    let copy = file.copy?;
    let id = file.id()?;
    is_bundled(&id).then_some((id, copy))
}

/// The top-level `.md` files of `agents_dir` that [`rescan_file_layer`]
/// sets aside against this build's bundled templates, in file-name order,
/// each with the copy it is — what `openalpaca agents tidy` lists and moves
/// (subagent design §2.12 D-2). Reads only; a directory that does not exist
/// holds none.
pub fn set_aside_copies(agents_dir: &Path) -> std::io::Result<Vec<(PathBuf, ShippedCopy)>> {
    let bundled: Vec<String> = crate::bundled::bundled_templates()
        .into_iter()
        .map(|template| template.frontmatter.id)
        .collect();
    Ok(read_template_files(agents_dir)?
        .into_iter()
        .filter_map(|file| {
            let (_, copy) = set_aside_as(&file, |id| bundled.iter().any(|b| b == id))?;
            Some((file.path, copy))
        })
        .collect())
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
            let bytes = std::fs::read(&path);
            let copy = bytes
                .as_ref()
                .ok()
                .and_then(|bytes| classify_agent_file(bytes));
            let content = match bytes {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) if lenient_disabled(&text) => Content::Disabled {
                        id: lenient_template_id(&text),
                    },
                    Ok(text) => match parse_agent_markdown(&text) {
                        Ok(template) => Content::Template(Box::new(template)),
                        Err(e) => Content::Broken(Broken {
                            id: lenient_template_id(&text),
                            error: e.to_string(),
                        }),
                    },
                    Err(e) => Content::Broken(Broken {
                        id: lenient_template_id(&String::from_utf8_lossy(e.as_bytes())),
                        error: "the file is not UTF-8".to_string(),
                    }),
                },
                Err(e) => Content::Broken(Broken {
                    id: None,
                    error: format!("cannot be read: {e}"),
                }),
            };
            Read {
                path,
                copy,
                content,
            }
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
    // The upsert never moves an existing row's status, so a row a DELETE
    // archived — an earlier build's, before the bundled template of its id
    // ran again, or this build's, before a file of its id came back — or one
    // a switching file left `disabled` (L-4a) would read so while the
    // template runs: set it back.
    if let Ok(Some(row)) = repo.get(template_id)
        && (row.status == "archived" || row.status == "disabled")
    {
        let _ = repo.update_status(template_id, "idle", None);
    }

    // Initialize metrics row if not exists
    if let Ok(None) = repo.get_metrics(template_id) {
        let _ = repo.upsert_metrics(&openalpaca_storage::AgentMetrics::new_empty(template_id));
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bundle_tests;

#[cfg(test)]
mod disabled_tests;
