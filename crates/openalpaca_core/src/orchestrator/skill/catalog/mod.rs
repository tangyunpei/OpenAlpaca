//! Skill Catalog: progressive loading of SKILL.md-based skills.
//!
//! Level 1 (startup): Scans skill directories and loads only YAML frontmatter
//! for each SKILL.md file, building a lightweight in-memory catalog.
//!
//! Level 2 (on-demand): Loads the full SKILL.md body when a skill is invoked,
//! providing the complete instructions and templates.
//!
//! Skills are keyed by their **directory name** (the skill ID). The catalog
//! supports multi-scope discovery: project-level skills can override user-level
//! skills with the same ID.

use crate::bus::EventBus;
use crate::events::SystemEvent;
use crate::middleware::skill::{
    SkillDocument, SkillFrontmatter, SkillScope, parse_skill_frontmatter, parse_skill_markdown,
};
use chrono::Utc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Where a skill's execution logic comes from.
#[derive(Clone)]
pub enum SkillSource {
    /// Traditional file-based skill (SKILL.md)
    FileBased,
    /// Plugin-backed skill (executed via PluginSkillExecutor)
    Plugin {
        plugin_id: String,
        executor: Arc<dyn openalpaca_api::plugin_traits::PluginSkillExecutor>,
    },
}

impl std::fmt::Debug for SkillSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileBased => write!(f, "FileBased"),
            Self::Plugin { plugin_id, .. } => write!(f, "Plugin({plugin_id})"),
        }
    }
}

/// A catalog entry: Level 1 metadata + path for deferred Level 2 loading.
#[derive(Debug, Clone)]
pub struct SkillEntry {
    /// Parsed frontmatter (always available after catalog scan).
    pub frontmatter: SkillFrontmatter,
    /// Absolute path to the SKILL.md file (None for plugin skills).
    pub skill_md_path: Option<PathBuf>,
    /// Absolute path to the skill directory (None for plugin skills).
    pub skill_dir: Option<PathBuf>,
    /// Where this skill was discovered.
    pub scope: SkillScope,
    /// Where this skill's execution logic comes from.
    pub source: SkillSource,
}

/// Cached catalog summary row. The skill **id** rides along so the
/// availability filter (design §6.2 #12) can re-test each row against the
/// oracle on every read: the cache is invalidated by catalog mutations, and an
/// extension toggle is not one.
#[derive(Clone)]
struct CatalogSummaryRow {
    id: String,
    name: String,
    description: String,
    command: Option<String>,
}

/// Cached catalog summary.
type CatalogSummaryCache = RwLock<Option<Vec<CatalogSummaryRow>>>;

/// What a plugin used to contribute, kept after T2 withdrew it so `/slash` and
/// `invoke_skill` can answer *"skill 'x' is provided by plugin 'notion', which
/// is disabled"* instead of *"unknown skill"* plus a dump of every catalog name
/// (design §10 case 5(a)).
#[derive(Debug, Clone)]
pub struct SkillTombstone {
    /// The lowercased skill id the entry was removed under.
    pub skill_id: String,
    /// The plugin directory name — the ledger key its state is read from.
    pub plugin_id: String,
}

/// Central skill catalog — lightweight at startup, loads full content on demand.
///
/// Thread-safe via internal `RwLock`. All mutations go through `&self` methods.
///
/// Entries are keyed by **directory name** (skill ID), not frontmatter name.
pub struct SkillCatalog {
    /// All known skills, keyed by directory name (skill ID, lowercase).
    entries: RwLock<HashMap<String, SkillEntry>>,
    /// Slash command index: maps command (e.g. "review") → skill ID.
    command_index: RwLock<HashMap<String, String>>,
    /// Alias index: maps alias command → skill ID.
    alias_index: RwLock<HashMap<String, String>>,
    /// Diagnostic messages accumulated during scanning.
    /// Optional event bus for emitting lifecycle events.
    bus: Option<EventBus>,
    /// Cached catalog summary — invalidated on mutation (Opt-8a).
    cached_summary: CatalogSummaryCache,
    /// The ENABLE axis's availability question (design §6.2 #12). `SkillRouter`
    /// takes only `(&str, &SkillCatalog)` and has no registry handle, so the
    /// catalog carries the oracle for it, for `catalog_summary` and for the
    /// `invoke_skill` listing. `None` — every test catalog, and the window
    /// before the registry exists — means *everything is available*, which is
    /// the same fail-open the ledger takes for an unrecorded extension (§6.2a).
    availability_oracle: RwLock<Option<Arc<dyn crate::tools::registry::CapabilityOracle>>>,
    /// Withdrawn plugin contributions, keyed by the lowercased skill id **and**
    /// by the slash command and aliases captured before `remove` scrubbed the
    /// indices (design §10 case 5(a)).
    tombstones: RwLock<HashMap<String, SkillTombstone>>,
}

impl Default for SkillCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillCatalog {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            command_index: RwLock::new(HashMap::new()),
            alias_index: RwLock::new(HashMap::new()),
            bus: None,
            cached_summary: RwLock::new(None),
            availability_oracle: RwLock::new(None),
            tombstones: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new SkillCatalog with an event bus for lifecycle events.
    pub fn new_with_bus(bus: EventBus) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            command_index: RwLock::new(HashMap::new()),
            alias_index: RwLock::new(HashMap::new()),
            bus: Some(bus),
            cached_summary: RwLock::new(None),
            availability_oracle: RwLock::new(None),
            tombstones: RwLock::new(HashMap::new()),
        }
    }

    /// Install the ENABLE axis's availability oracle — `ToolRegistry`
    /// (design §6.2 #12). Wired once, in the daemon's service bundle, as soon
    /// as both exist.
    pub fn set_availability_oracle(
        &self,
        oracle: Arc<dyn crate::tools::registry::CapabilityOracle>,
    ) {
        if let Ok(mut guard) = self.availability_oracle.write() {
            *guard = Some(oracle);
        }
        self.invalidate_summary_cache();
    }

    /// **The one predicate** (design §10 case 3, §6.2 #12): can this skill
    /// still reach every requirement it declares? With no oracle installed the
    /// answer is always yes — nothing is known to have been withdrawn.
    pub fn is_satisfiable(&self, frontmatter: &SkillFrontmatter) -> bool {
        match self.availability_oracle.read() {
            Ok(guard) => guard
                .as_ref()
                .is_none_or(|oracle| oracle.is_satisfiable(frontmatter)),
            Err(_) => true,
        }
    }

    /// Skill IDs whose requirements are still served — the `invoke_skill`
    /// listing (design §6.2 #12). A skill this omits is one no `/slash` and no
    /// `invoke_skill` would run.
    pub fn available_names(&self) -> Vec<String> {
        match self.entries.read() {
            Ok(guard) => guard
                .iter()
                .filter(|(_, entry)| self.is_satisfiable(&entry.frontmatter))
                .map(|(id, _)| id.clone())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    // ── Tombstones for withdrawn plugin contributions (§10 case 5(a)) ────

    /// Remove a plugin's skill and leave a tombstone behind.
    ///
    /// `remove` scrubs the command and alias indices, so afterwards
    /// `get_by_command` resolves to nothing and falls through to a `get_by_id`
    /// that also misses. The tombstone is a **separate** map — the live indices
    /// stay scrubbed exactly as today — keyed by the lowercased id and by the
    /// command and aliases captured *before* the removal, and consulted by the
    /// `/slash` tier and `invoke_skill` **only on a miss**.
    pub fn remove_plugin_skill(&self, skill_id: &str, plugin_id: &str) {
        let key = skill_id.to_lowercase();
        let mut keys = vec![key.clone()];
        if let Ok(entries) = self.entries.read()
            && let Some(entry) = entries.get(&key)
        {
            if let Some(cmd) = entry.frontmatter.effective_slash_command() {
                keys.push(cmd.strip_prefix('/').unwrap_or(&cmd).to_lowercase());
            }
            for alias in &entry.frontmatter.invoke.aliases {
                keys.push(alias.strip_prefix('/').unwrap_or(alias).to_lowercase());
            }
        }
        self.remove(skill_id);
        if let Ok(mut tombstones) = self.tombstones.write() {
            for k in keys {
                tombstones.insert(
                    k,
                    SkillTombstone {
                        skill_id: key.clone(),
                        plugin_id: plugin_id.to_string(),
                    },
                );
            }
        }
    }

    /// The tombstone for a skill id, slash command or alias, if one was left.
    pub fn tombstone(&self, name: &str) -> Option<SkillTombstone> {
        let key = name.strip_prefix('/').unwrap_or(name).to_lowercase();
        self.tombstones.read().ok()?.get(&key).cloned()
    }

    /// Drop every tombstone belonging to a skill that has just come back, plus
    /// any keyed by a command the new entry claims.
    fn clear_tombstones(&self, skill_id: &str, commands: &[String]) {
        let key = skill_id.to_lowercase();
        if let Ok(mut tombstones) = self.tombstones.write() {
            tombstones.retain(|k, tomb| {
                tomb.skill_id != key && !commands.iter().any(|c| c == k)
            });
        }
    }

    /// Scan a directory for skill folders containing SKILL.md.
    ///
    /// Each direct child directory that contains a `SKILL.md` file is treated
    /// as a skill. Only the YAML frontmatter is parsed (Level 1).
    /// Returns the count of skills successfully loaded.
    pub fn scan_directory(&self, dir: &Path, scope: SkillScope) -> usize {
        let read_dir = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) => {
                tracing::warn!(
                    "SkillCatalog: failed to read skills directory {}: {}",
                    dir.display(),
                    e
                );
                return 0;
            }
        };

        let mut count = 0usize;

        for entry in read_dir.flatten() {
            let child_path = entry.path();
            if !child_path.is_dir() {
                continue;
            }
            let skill_md = child_path.join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }

            match self.load_entry(&skill_md, &child_path, scope) {
                Ok(()) => count += 1,
                Err(e) => {
                    tracing::warn!("SkillCatalog: failed to load {}: {}", skill_md.display(), e);
                }
            }
        }

        count
    }

    /// Scan multiple scope directories in priority order.
    ///
    /// User-scope skills are loaded first, then project-scope skills override
    /// any user-scope skills with the same directory name (skill ID).
    pub fn scan_multi_scope(&self, user_dir: Option<&Path>, project_dir: Option<&Path>) -> usize {
        let mut total = 0;
        if let Some(dir) = user_dir
            && dir.exists()
        {
            total += self.scan_directory(dir, SkillScope::User);
        }
        if let Some(dir) = project_dir
            && dir.exists()
        {
            total += self.scan_directory(dir, SkillScope::Project);
        }
        self.validate_dependencies();
        total
    }

    /// Load a single SKILL.md entry (Level 1 — frontmatter only).
    fn load_entry(
        &self,
        skill_md: &Path,
        skill_dir: &Path,
        scope: SkillScope,
    ) -> Result<(), String> {
        let dir_name = skill_dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "invalid directory name".to_string())?;

        let content =
            std::fs::read_to_string(skill_md).map_err(|e| format!("read error: {}", e))?;

        let frontmatter =
            parse_skill_frontmatter(&content).map_err(|e| format!("parse error: {}", e))?;

        // Deprecation warnings for parsed-but-dead fields
        if frontmatter.context.summarize.enabled {
            let msg = format!(
                "Skill '{}': 'context.summarize.enabled' is deprecated and has no effect. \
                 Use 'context.budget_tokens' for context size control.",
                frontmatter.name
            );
            tracing::warn!("SkillCatalog: {}", msg);
        }
        if !frontmatter.tools.defaults.is_empty() {
            let msg = format!(
                "Skill '{}': 'tools.defaults' is deprecated and has no effect. \
                 Tool default arguments are not injected at runtime.",
                frontmatter.name
            );
            tracing::warn!("SkillCatalog: {}", msg);
        }

        // Validation: invoke.mode == "disabled" => skip
        if frontmatter.invoke.mode == "disabled" {
            tracing::info!(
                "SkillCatalog: skipping disabled skill '{}'",
                frontmatter.name
            );
            return Ok(());
        }

        // Validation: if frontmatter.id is set, warn if it doesn't match directory name
        if let Some(ref declared_id) = frontmatter.id
            && declared_id != dir_name
        {
            let msg = format!(
                "Skill '{}': frontmatter id '{}' does not match directory name '{}'",
                frontmatter.name, declared_id, dir_name
            );
            tracing::warn!("SkillCatalog: {}", msg);
        }

        let key = dir_name.to_lowercase();
        let skill_name_for_event = frontmatter.name.clone();

        let entry = SkillEntry {
            frontmatter,
            skill_md_path: Some(skill_md.to_path_buf()),
            skill_dir: Some(skill_dir.to_path_buf()),
            scope,
            source: SkillSource::FileBased,
        };

        // Collect old index data and insert entry under entries lock,
        // then release entries lock before acquiring index locks to avoid deadlock.
        let (old_slash_cmd, old_aliases, new_slash_cmd, new_aliases) = {
            let mut entries_guard = match self.entries.write() {
                Ok(g) => g,
                Err(_) => {
                    tracing::error!(skill = %key, "Failed to acquire entries write lock — skill not loaded");
                    return Err("entries lock poisoned".to_string());
                }
            };

            // Collect old entry's index data for cleanup
            let (old_slash, old_als) = if let Some(old_entry) = entries_guard.get(&key) {
                (
                    old_entry.frontmatter.effective_slash_command(),
                    old_entry.frontmatter.invoke.aliases.clone(),
                )
            } else {
                (None, Vec::new())
            };

            // Collect new entry's index data
            let new_slash = entry.frontmatter.effective_slash_command();
            let new_als = entry.frontmatter.invoke.aliases.clone();

            entries_guard.insert(key.clone(), entry);

            // entries_guard is dropped here (end of block)
            (old_slash, old_als, new_slash, new_als)
        };

        // Remove old index entries (entries lock released)
        self.remove_index_entries(old_slash_cmd.as_deref(), &old_aliases);

        // Build command index from effective_slash_command (entries lock released)
        if let Some(ref cmd) = new_slash_cmd {
            if let Ok(mut idx) = self.command_index.write() {
                let cmd_lower = cmd.to_lowercase();
                // Check for slash command conflicts
                if let Some(existing_key) = idx.get(&cmd_lower)
                    && *existing_key != key
                {
                    let msg = format!(
                        "Slash command '{}' conflict: skill '{}' overrides '{}'",
                        cmd, key, existing_key
                    );
                    tracing::warn!("SkillCatalog: {}", msg);
                }
                idx.insert(cmd_lower, key.clone());
            }
        }
        // Build alias index (entries lock released)
        if let Ok(mut alias_idx) = self.alias_index.write() {
            for alias in &new_aliases {
                let alias_lower = alias.strip_prefix('/').unwrap_or(alias).to_lowercase();
                alias_idx.insert(alias_lower, key.clone());
            }
        }

        // A skill that came back invalidates its own tombstone.
        {
            let mut claimed: Vec<String> = new_aliases
                .iter()
                .map(|a| a.strip_prefix('/').unwrap_or(a).to_lowercase())
                .collect();
            if let Some(ref cmd) = new_slash_cmd {
                claimed.push(cmd.strip_prefix('/').unwrap_or(cmd).to_lowercase());
            }
            self.clear_tombstones(&key, &claimed);
        }

        // Emit lifecycle event
        if let Some(ref bus) = self.bus {
            let scope_str = match scope {
                SkillScope::Project => "project",
                SkillScope::User => "user",
            };
            bus.publish(SystemEvent::SkillDiscovered {
                skill_id: key,
                skill_name: skill_name_for_event,
                scope: scope_str.to_string(),
                timestamp: Utc::now(),
            });
        }

        self.invalidate_summary_cache();
        Ok(())
    }

    /// Remove command_index and alias_index entries for a skill's frontmatter data.
    ///
    /// Accepts extracted frontmatter fields so that the entries lock does NOT need
    /// to be held while the index locks are acquired, preventing deadlocks.
    fn remove_index_entries(&self, slash_cmd: Option<&str>, aliases: &[String]) {
        if let Some(cmd) = slash_cmd {
            if let Ok(mut idx) = self.command_index.write() {
                idx.remove(&cmd.to_lowercase());
            }
        }
        if let Ok(mut alias_idx) = self.alias_index.write() {
            for alias in aliases {
                let alias_lower = alias.strip_prefix('/').unwrap_or(alias).to_lowercase();
                alias_idx.remove(&alias_lower);
            }
        }
    }

    /// Look up a skill by ID (directory name) or by frontmatter name (case-insensitive).
    ///
    /// Tries ID lookup first (O(1)), then falls back to scanning by name.
    pub fn get(&self, name: &str) -> Option<SkillEntry> {
        let guard = self.entries.read().ok()?;
        let lower = name.to_lowercase();
        // Try direct ID lookup
        if let Some(entry) = guard.get(&lower) {
            return Some(entry.clone());
        }
        // Fallback: search by frontmatter name
        guard
            .values()
            .find(|e| e.frontmatter.name.to_lowercase() == lower)
            .cloned()
    }

    /// Look up a skill by slash command (e.g. "review" -> SkillEntry).
    ///
    /// Checks the primary command index first, then the alias index, then
    /// falls back to a direct skill-ID lookup so `/{skill_id}` always works —
    /// scheduled skills (invoke.cron) rely on this when they declare no
    /// explicit slash command. Explicit commands and aliases win on conflict.
    pub fn get_by_command(&self, command: &str) -> Option<SkillEntry> {
        let cmd_lower = command.to_lowercase();
        // Snapshot both indices in one scope to avoid stale results from
        // concurrent mutations between the two reads.
        let (cmd_id, alias_id) = {
            let cmd_guard = self.command_index.read().ok()?;
            let alias_guard = self.alias_index.read().ok()?;
            (
                cmd_guard.get(&cmd_lower).cloned(),
                alias_guard.get(&cmd_lower).cloned(),
            )
        };
        if let Some(id) = cmd_id {
            if let Some(entry) = self.get_by_id(&id) {
                return Some(entry);
            }
        }
        if let Some(id) = alias_id
            && let Some(entry) = self.get_by_id(&id)
        {
            return Some(entry);
        }
        // Fallback: treat the command as a skill ID (directory name).
        self.get_by_id(&cmd_lower)
    }

    /// Look up a skill by its ID (directory name) only.
    fn get_by_id(&self, id: &str) -> Option<SkillEntry> {
        let guard = self.entries.read().ok()?;
        guard.get(&id.to_lowercase()).cloned()
    }

    /// List all registered skill IDs (directory names).
    pub fn list_names(&self) -> Vec<String> {
        match self.entries.read() {
            Ok(guard) => guard.keys().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    /// List all skills with their (name, description, command) for prompt catalog.
    /// Returns a cached copy when available; rebuilt on catalog mutations.
    ///
    /// **Availability-filtered** (design §6.2 #12): a skill any of whose
    /// required capabilities is wholly withheld is dropped, so
    /// `<available_skills>` never coaches the model toward a skill `/slash`
    /// would refuse. The filter runs on **read**, not into the cache — the
    /// cache is invalidated by catalog mutations, and an extension toggle is
    /// not one.
    pub fn catalog_summary(&self) -> Vec<(String, String, Option<String>)> {
        // Fast path: return cached summary
        let cached = {
            let guard = self.cached_summary.read().ok();
            guard.and_then(|g| g.clone())
        };
        let rows = match cached {
            Some(rows) => rows,
            None => {
                // Slow path: build summary and cache it
                let rows = match self.entries.read() {
                    Ok(guard) => {
                        let mut rows: Vec<CatalogSummaryRow> = guard
                            .iter()
                            .map(|(id, e)| CatalogSummaryRow {
                                id: id.clone(),
                                name: e.frontmatter.name.clone(),
                                description: e.frontmatter.description.clone(),
                                command: e.frontmatter.effective_slash_command(),
                            })
                            .collect();
                        rows.sort_by(|a, b| a.name.cmp(&b.name));
                        rows
                    }
                    Err(_) => Vec::new(),
                };
                if let Ok(mut cache) = self.cached_summary.write() {
                    *cache = Some(rows.clone());
                }
                rows
            }
        };

        let to_tuple = |row: CatalogSummaryRow| (row.name, row.description, row.command);
        if !self.has_availability_oracle() {
            return rows.into_iter().map(to_tuple).collect();
        }

        let unavailable: std::collections::HashSet<String> = match self.entries.read() {
            Ok(guard) => guard
                .iter()
                .filter(|(_, entry)| !self.is_satisfiable(&entry.frontmatter))
                .map(|(id, _)| id.clone())
                .collect(),
            Err(_) => std::collections::HashSet::new(),
        };

        rows.into_iter()
            .filter(|row| !unavailable.contains(&row.id))
            .map(to_tuple)
            .collect()
    }

    /// Is an oracle installed at all? With none, nothing is known to have been
    /// withdrawn and every availability filter is a no-op.
    fn has_availability_oracle(&self) -> bool {
        self.availability_oracle
            .read()
            .is_ok_and(|guard| guard.is_some())
    }

    /// Invalidate the cached catalog summary (call after any mutation).
    fn invalidate_summary_cache(&self) {
        if let Ok(mut cache) = self.cached_summary.write() {
            *cache = None;
        }
    }

    /// Load full skill content (Level 2) from disk.
    ///
    /// Re-reads and fully parses the SKILL.md file. Does not cache the result —
    /// the caller decides how long to keep it.
    pub fn load_full(&self, name: &str) -> Result<SkillDocument, String> {
        let entry = self
            .get(name)
            .ok_or_else(|| format!("Skill '{}' not found in catalog", name))?;

        if entry.skill_md_path.is_none() {
            // Plugin skill — return synthetic document
            return Ok(SkillDocument {
                frontmatter: entry.frontmatter.clone(),
                body: entry.frontmatter.description.clone(),
                sections: HashMap::new(),
            });
        }
        let path = entry.skill_md_path.as_ref()
            .expect("file-based skill must have skill_md_path");

        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

        parse_skill_markdown(&content)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
    }

    /// Remove a skill from the catalog by ID or name.
    pub fn remove(&self, name: &str) {
        let key = name.to_lowercase();
        // Extract frontmatter data under entries lock, then release before acquiring index locks.
        let removed_data = if let Ok(mut entries) = self.entries.write() {
            // Find the actual key — try direct ID, then search by name
            let actual_key = if entries.contains_key(&key) {
                Some(key.clone())
            } else {
                entries
                    .iter()
                    .find(|(_, e)| e.frontmatter.name.to_lowercase() == key)
                    .map(|(k, _)| k.clone())
            };

            if let Some(ref actual) = actual_key {
                let data = entries.get(actual).map(|entry| {
                    (
                        entry.frontmatter.effective_slash_command(),
                        entry.frontmatter.invoke.aliases.clone(),
                    )
                });
                entries.remove(actual);
                data
            } else {
                None
            }
        } else {
            None
        };

        // Clean up index entries after releasing entries lock
        if let Some((slash_cmd, aliases)) = removed_data {
            self.remove_index_entries(slash_cmd.as_deref(), &aliases);
        }
        self.invalidate_summary_cache();
    }

    /// Register a plugin-backed skill into the catalog.
    ///
    /// Plugin skills have no filesystem path — their execution is delegated
    /// to the `PluginSkillExecutor` provided by the plugin runtime.
    ///
    /// **The id is lowercased at insert** (design §6.2 #14). Every reader
    /// lowercases: `get_by_id` looks up `id.to_lowercase()` with no name
    /// fallback, so a plugin whose `skill/info` returns a mixed-case id was
    /// unreachable by `/slash` and by `invoke_skill`; and `remove(skill_id)`
    /// found the verbatim entry only through its name-scan fallback, so a
    /// plugin supplying both an id slug and a display name leaked a catalog
    /// entry holding an executor for a killed process at every unload.
    pub fn register_plugin_skill(
        &self,
        skill_id: String,
        frontmatter: SkillFrontmatter,
        executor: Arc<dyn openalpaca_api::plugin_traits::PluginSkillExecutor>,
        plugin_id: String,
    ) {
        let skill_id = skill_id.to_lowercase();
        let slash_cmd = frontmatter.invoke.slash.clone();
        let aliases = frontmatter.invoke.aliases.clone();
        let entry = SkillEntry {
            frontmatter,
            skill_md_path: None,
            skill_dir: None,
            scope: SkillScope::User,
            source: SkillSource::Plugin { plugin_id, executor },
        };

        // Insert entry under entries lock, then release before acquiring index locks
        {
            let mut entries = self.entries.write().unwrap_or_else(|p| {
                tracing::error!("SkillCatalog lock poisoned — recovering");
                p.into_inner()
            });
            entries.insert(skill_id.clone(), entry);
        }

        // Update command index (entries lock released)
        // Normalize: strip leading "/" and lowercase to match get_by_command() lookup.
        if let Some(ref cmd) = slash_cmd {
            let mut cmd_idx = self.command_index.write().unwrap_or_else(|p| {
                tracing::error!("SkillCatalog lock poisoned — recovering");
                p.into_inner()
            });
            let cmd_lower = cmd.strip_prefix('/').unwrap_or(cmd).to_lowercase();
            cmd_idx.insert(cmd_lower, skill_id.clone());
        }
        // Update alias index (entries lock released)
        // Normalize: strip leading "/" and lowercase to match get_by_command() lookup.
        if !aliases.is_empty() {
            let mut alias_idx = self.alias_index.write().unwrap_or_else(|p| {
                tracing::error!("SkillCatalog lock poisoned — recovering");
                p.into_inner()
            });
            for alias in &aliases {
                let alias_lower = alias.strip_prefix('/').unwrap_or(alias).to_lowercase();
                alias_idx.insert(alias_lower, skill_id.clone());
            }
        }
        // A re-enabled plugin's skill is live again — drop its tombstone.
        {
            let mut claimed: Vec<String> = aliases
                .iter()
                .map(|a| a.strip_prefix('/').unwrap_or(a).to_lowercase())
                .collect();
            if let Some(ref cmd) = slash_cmd {
                claimed.push(cmd.strip_prefix('/').unwrap_or(cmd).to_lowercase());
            }
            self.clear_tombstones(&skill_id, &claimed);
        }
        self.invalidate_summary_cache();
    }

    /// Hot-reload a single skill directory.
    ///
    /// Removes the old entry (if any) and re-scans the directory.
    /// Preserves the original scope if the skill already existed.
    pub fn reload_skill(&self, skill_dir: &Path) -> Result<(), String> {
        let skill_md = skill_dir.join("SKILL.md");

        // Determine the scope of the existing entry (default to Project)
        let existing_scope = skill_dir
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|dir_name| {
                self.entries
                    .read()
                    .ok()
                    .and_then(|guard| guard.get(&dir_name.to_lowercase()).map(|e| e.scope))
            })
            .unwrap_or(SkillScope::Project);

        if !skill_md.exists() {
            // Skill was deleted — remove from catalog
            if let Some(dir_name) = skill_dir.file_name().and_then(|n| n.to_str()) {
                // Try to find and remove by directory name
                let skill_dir_buf = skill_dir.to_path_buf();
                let entries_to_remove: Vec<String> = match self.entries.read() {
                    Ok(guard) => guard
                        .iter()
                        .filter(|(_, e)| e.skill_dir.as_ref() == Some(&skill_dir_buf))
                        .map(|(k, _)| k.clone())
                        .collect(),
                    Err(_) => return Err("Lock poisoned".to_string()),
                };
                for key in entries_to_remove {
                    self.remove(&key);
                }
                tracing::info!("SkillCatalog: removed skill from {}", dir_name);
            }
            return Ok(());
        }

        // Read frontmatter to get the name for logging
        let content =
            std::fs::read_to_string(&skill_md).map_err(|e| format!("read error: {}", e))?;
        let fm = parse_skill_frontmatter(&content).map_err(|e| format!("parse error: {}", e))?;

        // Remove old entries for this directory (handles renames)
        let skill_dir_buf = skill_dir.to_path_buf();
        let old_keys: Vec<String> = match self.entries.read() {
            Ok(guard) => guard
                .iter()
                .filter(|(_, e)| e.skill_dir.as_ref() == Some(&skill_dir_buf))
                .map(|(k, _)| k.clone())
                .collect(),
            Err(_) => Vec::new(),
        };
        for key in old_keys {
            self.remove(&key);
        }

        // Re-load with preserved scope
        self.load_entry(&skill_md, skill_dir, existing_scope)?;
        tracing::info!("SkillCatalog: reloaded skill '{}'", fm.name);
        Ok(())
    }

    /// Number of registered skills.
    pub fn count(&self) -> usize {
        match self.entries.read() {
            Ok(guard) => guard.len(),
            Err(_) => 0,
        }
    }

    /// Check if the catalog is empty.
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// Return a snapshot of all entries for external iteration (e.g. by SkillRouter).
    pub fn entries_snapshot(&self) -> Vec<(String, SkillEntry)> {
        match self.entries.read() {
            Ok(guard) => guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Validate that all `depends_on` references exist and contain no cycles.
    /// Call after `scan_directory()` or `scan_multi_scope()`.
    pub fn validate_dependencies(&self) -> Vec<String> {
        use std::collections::HashSet;

        let entries = self
            .entries
            .read()
            .unwrap_or_else(|p| {
                tracing::error!("SkillCatalog lock poisoned — recovering");
                p.into_inner()
            });
        let mut errors = Vec::new();

        // Phase 1: Check existence of all dependency references
        for (id, entry) in entries.iter() {
            for dep_id in &entry.frontmatter.depends_on {
                if !entries.contains_key(dep_id) {
                    errors.push(format!(
                        "Skill '{}' depends on '{}' which does not exist",
                        id, dep_id
                    ));
                }
            }
        }

        // Phase 2: Three-color DFS cycle detection
        // White = not visited, Gray = in current path, Black = fully explored
        #[derive(Clone, Copy, PartialEq)]
        enum Color {
            White,
            Gray,
            Black,
        }

        let mut color: HashMap<&str, Color> =
            entries.keys().map(|k| (k.as_str(), Color::White)).collect();
        let mut reported_cycles: HashSet<String> = HashSet::new();

        // Use iterative DFS to avoid lifetime issues with recursive functions
        for start_id in entries.keys() {
            if color.get(start_id.as_str()) != Some(&Color::White) {
                continue;
            }

            // Stack holds (node, iterator index into depends_on)
            let mut stack: Vec<(&str, usize)> = vec![(start_id.as_str(), 0)];
            color.insert(start_id.as_str(), Color::Gray);

            while let Some((node, idx)) = stack.last_mut() {
                let deps = entries
                    .get(*node)
                    .map(|e| &e.frontmatter.depends_on)
                    .cloned()
                    .unwrap_or_default();

                if *idx >= deps.len() {
                    // All neighbors explored — mark black
                    color.insert(*node, Color::Black);
                    stack.pop();
                    continue;
                }

                let dep = deps[*idx].clone();
                *idx += 1;

                // Resolve to the key stored in entries (stable &str lifetime)
                let dep_key = match entries.get_key_value(dep.as_str()) {
                    Some((k, _)) => k.as_str(),
                    None => continue, // Already reported in Phase 1
                };

                match color.get(dep_key) {
                    Some(Color::Gray) => {
                        // Back edge — cycle found. Build full cycle path from
                        // the stack: find where dep_key first appears and trace
                        // through to the current node.
                        let mut cycle_path = Vec::new();
                        let mut in_cycle = false;
                        for (stack_node, _) in &stack {
                            if *stack_node == dep_key {
                                in_cycle = true;
                            }
                            if in_cycle {
                                cycle_path.push(*stack_node);
                            }
                        }
                        cycle_path.push(dep_key); // close the cycle
                        let msg = format!(
                            "Cycle detected: {}",
                            cycle_path
                                .iter()
                                .map(|s| format!("'{}'", s))
                                .collect::<Vec<_>>()
                                .join(" -> ")
                        );
                        if reported_cycles.insert(msg.clone()) {
                            errors.push(msg);
                        }
                    }
                    Some(Color::White) | None => {
                        color.insert(dep_key, Color::Gray);
                        stack.push((dep_key, 0));
                    }
                    Some(Color::Black) => {} // Already fully explored
                }
            }
        }

        if !errors.is_empty() {
            for err in &errors {
                tracing::warn!("{}", err);
            }
        }

        errors
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
