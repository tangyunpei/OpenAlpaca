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
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// A catalog entry: Level 1 metadata + path for deferred Level 2 loading.
#[derive(Debug, Clone)]
pub struct SkillEntry {
    /// Parsed frontmatter (always available after catalog scan).
    pub frontmatter: SkillFrontmatter,
    /// Absolute path to the SKILL.md file.
    pub skill_md_path: PathBuf,
    /// Absolute path to the skill directory (parent of SKILL.md).
    pub skill_dir: PathBuf,
    /// Compiled trigger regex patterns (compiled once at scan time).
    /// Patterns that fail to compile are silently dropped.
    pub compiled_triggers: Vec<Regex>,
    /// Where this skill was discovered.
    pub scope: SkillScope,
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
    validation_errors: RwLock<Vec<String>>,
    /// Optional event bus for emitting lifecycle events.
    bus: Option<EventBus>,
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
            validation_errors: RwLock::new(Vec::new()),
            bus: None,
        }
    }

    /// Create a new SkillCatalog with an event bus for lifecycle events.
    pub fn new_with_bus(bus: EventBus) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            command_index: RwLock::new(HashMap::new()),
            alias_index: RwLock::new(HashMap::new()),
            validation_errors: RwLock::new(Vec::new()),
            bus: Some(bus),
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
            if let Ok(mut errors) = self.validation_errors.write() {
                errors.push(msg);
            }
        }

        // Compile trigger patterns (use routing.intent which is populated by legacy compat)
        let compiled_triggers: Vec<Regex> = frontmatter
            .routing
            .intent
            .iter()
            .filter_map(|pattern| match Regex::new(&format!("(?i){}", pattern)) {
                Ok(re) => Some(re),
                Err(e) => {
                    tracing::warn!(
                        "SkillCatalog: invalid trigger pattern '{}' in {}: {}",
                        pattern,
                        frontmatter.name,
                        e
                    );
                    None
                }
            })
            .collect();

        let key = dir_name.to_lowercase();
        let skill_name_for_event = frontmatter.name.clone();

        let entry = SkillEntry {
            frontmatter,
            skill_md_path: skill_md.to_path_buf(),
            skill_dir: skill_dir.to_path_buf(),
            compiled_triggers,
            scope,
        };

        // Insert entry and command/alias indices
        if let Ok(mut entries) = self.entries.write() {
            // If overriding an existing entry, clean up its old command/alias index entries
            if let Some(old_entry) = entries.get(&key) {
                self.remove_index_entries_for(old_entry);
            }

            // Build command index from effective_slash_command
            if let Some(ref cmd) = entry.frontmatter.effective_slash_command()
                && let Ok(mut idx) = self.command_index.write()
            {
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
                    if let Ok(mut errors) = self.validation_errors.write() {
                        errors.push(msg);
                    }
                }
                idx.insert(cmd_lower, key.clone());
            }

            // Build alias index
            if let Ok(mut alias_idx) = self.alias_index.write() {
                for alias in &entry.frontmatter.invoke.aliases {
                    let alias_lower = alias
                        .strip_prefix('/')
                        .unwrap_or(alias)
                        .to_lowercase();
                    alias_idx.insert(alias_lower, key.clone());
                }
            }

            entries.insert(key.clone(), entry);
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

        Ok(())
    }

    /// Remove command_index and alias_index entries for a given skill entry.
    /// Must be called while entries write lock is held externally (caller responsibility).
    fn remove_index_entries_for(&self, entry: &SkillEntry) {
        if let Some(ref cmd) = entry.frontmatter.effective_slash_command()
            && let Ok(mut idx) = self.command_index.write()
        {
            idx.remove(&cmd.to_lowercase());
        }
        if let Ok(mut alias_idx) = self.alias_index.write() {
            for alias in &entry.frontmatter.invoke.aliases {
                let alias_lower = alias
                    .strip_prefix('/')
                    .unwrap_or(alias)
                    .to_lowercase();
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
    /// Checks the primary command index first, then the alias index.
    pub fn get_by_command(&self, command: &str) -> Option<SkillEntry> {
        let cmd_lower = command.to_lowercase();
        // Try primary command index
        let skill_id = {
            let guard = self.command_index.read().ok()?;
            guard.get(&cmd_lower).cloned()
        };
        if let Some(id) = skill_id
            && let Some(entry) = self.get_by_id(&id)
        {
            return Some(entry);
        }
        // Try alias index
        let alias_id = {
            let guard = self.alias_index.read().ok()?;
            guard.get(&cmd_lower).cloned()
        };
        if let Some(id) = alias_id {
            return self.get_by_id(&id);
        }
        None
    }

    /// Look up a skill by its ID (directory name) only.
    fn get_by_id(&self, id: &str) -> Option<SkillEntry> {
        let guard = self.entries.read().ok()?;
        guard.get(&id.to_lowercase()).cloned()
    }

    /// Find skills whose routing.intent patterns match the given text.
    ///
    /// Returns matched skill IDs, ordered by specificity (most patterns matched first).
    pub fn match_triggers(&self, text: &str) -> Vec<String> {
        let guard = match self.entries.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };

        let mut matches: Vec<(String, usize)> = Vec::new();

        for (key, entry) in guard.iter() {
            let hit_count = entry
                .compiled_triggers
                .iter()
                .filter(|re| re.is_match(text))
                .count();
            if hit_count > 0 {
                matches.push((key.clone(), hit_count));
            }
        }

        // Sort by hit count descending (most specific match first)
        matches.sort_by(|a, b| b.1.cmp(&a.1));
        matches.into_iter().map(|(name, _)| name).collect()
    }

    /// List all registered skill IDs (directory names).
    pub fn list_names(&self) -> Vec<String> {
        match self.entries.read() {
            Ok(guard) => guard.keys().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    /// List all skills with their (name, description, command) for prompt catalog.
    pub fn catalog_summary(&self) -> Vec<(String, String, Option<String>)> {
        match self.entries.read() {
            Ok(guard) => {
                let mut summaries: Vec<(String, String, Option<String>)> = guard
                    .values()
                    .map(|e| {
                        (
                            e.frontmatter.name.clone(),
                            e.frontmatter.description.clone(),
                            e.frontmatter.effective_slash_command(),
                        )
                    })
                    .collect();
                summaries.sort_by(|a, b| a.0.cmp(&b.0));
                summaries
            }
            Err(_) => Vec::new(),
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

        let content = std::fs::read_to_string(&entry.skill_md_path)
            .map_err(|e| format!("Failed to read {}: {}", entry.skill_md_path.display(), e))?;

        parse_skill_markdown(&content)
            .map_err(|e| format!("Failed to parse {}: {}", entry.skill_md_path.display(), e))
    }

    /// Remove a skill from the catalog by ID or name.
    pub fn remove(&self, name: &str) {
        let key = name.to_lowercase();
        if let Ok(mut entries) = self.entries.write() {
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
                // Clean up index entries before removing
                if let Some(entry) = entries.get(actual) {
                    self.remove_index_entries_for(entry);
                }
                entries.remove(actual);
            }
        }
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
                let entries_to_remove: Vec<String> = match self.entries.read() {
                    Ok(guard) => guard
                        .iter()
                        .filter(|(_, e)| e.skill_dir == skill_dir)
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
        let old_keys: Vec<String> = match self.entries.read() {
            Ok(guard) => guard
                .iter()
                .filter(|(_, e)| e.skill_dir == skill_dir)
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

    /// Return accumulated validation errors/warnings from scanning.
    pub fn validation_errors(&self) -> Vec<String> {
        match self.validation_errors.read() {
            Ok(guard) => guard.clone(),
            Err(_) => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::skill::SkillScope;
    use std::io::Write;
    use tempfile::TempDir;

    fn create_skill_dir(parent: &Path, name: &str, skill_md: &str) -> PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let md_path = dir.join("SKILL.md");
        let mut f = std::fs::File::create(&md_path).unwrap();
        f.write_all(skill_md.as_bytes()).unwrap();
        dir
    }

    const REVIEW_SKILL: &str = r#"---
name: "Code Review"
description: "Review code for bugs and style issues"
command: "review"
trigger_patterns:
  - "review.*code"
  - "code review"
tools_required:
  - "file_read"
auto_load: false
read_when:
  - "User asks for code review"
---

## Instructions

Analyze the code for bugs and style.
"#;

    const EXPLAIN_SKILL: &str = r#"---
name: "Explain Code"
description: "Explain what code does"
command: "explain-code"
trigger_patterns:
  - "explain.*code"
  - "what does.*do"
auto_load: false
---

## Instructions

Walk through the code step by step.
"#;

    #[test]
    fn test_scan_directory() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);
        create_skill_dir(tmp.path(), "explain-code", EXPLAIN_SKILL);

        let catalog = SkillCatalog::new();
        let count = catalog.scan_directory(tmp.path(), SkillScope::Project);
        assert_eq!(count, 2);
        assert_eq!(catalog.count(), 2);
    }

    #[test]
    fn test_get_by_id() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        // Lookup by directory name (skill ID)
        let entry = catalog.get("code-review").expect("should find by ID");
        assert_eq!(entry.frontmatter.name, "Code Review");
        assert_eq!(entry.scope, SkillScope::Project);
    }

    #[test]
    fn test_get_by_name_fallback() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        // Lookup by frontmatter name (fallback)
        let entry = catalog.get("Code Review").expect("should find by name");
        assert_eq!(entry.frontmatter.name, "Code Review");
        assert_eq!(entry.frontmatter.command, Some("review".to_string()));

        // Case insensitive
        let entry2 = catalog
            .get("code review")
            .expect("should find case-insensitive");
        assert_eq!(entry2.frontmatter.name, "Code Review");

        assert!(catalog.get("nonexistent").is_none());
    }

    #[test]
    fn test_get_by_command() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);
        create_skill_dir(tmp.path(), "explain-code", EXPLAIN_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let entry = catalog
            .get_by_command("review")
            .expect("should find /review");
        assert_eq!(entry.frontmatter.name, "Code Review");

        let entry2 = catalog
            .get_by_command("explain-code")
            .expect("should find /explain-code");
        assert_eq!(entry2.frontmatter.name, "Explain Code");

        assert!(catalog.get_by_command("nonexistent").is_none());
    }

    #[test]
    fn test_match_triggers() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);
        create_skill_dir(tmp.path(), "explain-code", EXPLAIN_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let matches = catalog.match_triggers("please review my code");
        assert!(!matches.is_empty());
        assert!(matches.contains(&"code-review".to_string()));

        let matches2 = catalog.match_triggers("explain this code to me");
        assert!(!matches2.is_empty());
        assert!(matches2.contains(&"explain-code".to_string()));

        let matches3 = catalog.match_triggers("hello world");
        assert!(matches3.is_empty());
    }

    #[test]
    fn test_load_full() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let doc = catalog.load_full("code-review").expect("should load full by ID");
        assert_eq!(doc.frontmatter.name, "Code Review");
        assert!(doc.body.contains("Analyze the code"));
        assert!(doc.sections.contains_key("Instructions"));

        // Also works by name fallback
        let doc2 = catalog.load_full("Code Review").expect("should load full by name");
        assert_eq!(doc2.frontmatter.name, "Code Review");
    }

    #[test]
    fn test_load_full_not_found() {
        let catalog = SkillCatalog::new();
        let result = catalog.load_full("nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_catalog_summary() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);
        create_skill_dir(tmp.path(), "explain-code", EXPLAIN_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let summary = catalog.catalog_summary();
        assert_eq!(summary.len(), 2);
        // Sorted by name
        assert_eq!(summary[0].0, "Code Review");
        assert_eq!(summary[1].0, "Explain Code");
        assert_eq!(summary[0].2, Some("review".to_string()));
    }

    #[test]
    fn test_remove() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);
        assert_eq!(catalog.count(), 1);
        assert!(catalog.get_by_command("review").is_some());

        // Remove by ID
        catalog.remove("code-review");
        assert_eq!(catalog.count(), 0);
        assert!(catalog.get_by_command("review").is_none());
    }

    #[test]
    fn test_remove_by_name() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);
        assert_eq!(catalog.count(), 1);

        // Remove by frontmatter name (backward compat)
        catalog.remove("Code Review");
        assert_eq!(catalog.count(), 0);
    }

    #[test]
    fn test_reload_skill() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let entry = catalog.get("code-review").unwrap();
        assert_eq!(
            entry.frontmatter.description,
            "Review code for bugs and style issues"
        );

        // Overwrite with updated content
        let updated = REVIEW_SKILL.replace(
            "Review code for bugs and style issues",
            "Updated description",
        );
        std::fs::write(skill_dir.join("SKILL.md"), updated).unwrap();

        catalog
            .reload_skill(&skill_dir)
            .expect("reload should succeed");

        let entry2 = catalog.get("code-review").unwrap();
        assert_eq!(entry2.frontmatter.description, "Updated description");
    }

    #[test]
    fn test_scan_ignores_non_directories() {
        let tmp = TempDir::new().unwrap();
        // Create a regular file (not a directory) in the skills dir
        std::fs::write(tmp.path().join("not_a_dir.md"), "hello").unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        let count = catalog.scan_directory(tmp.path(), SkillScope::Project);
        assert_eq!(count, 1);
    }

    #[test]
    fn test_scan_ignores_dirs_without_skill_md() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("empty-dir")).unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        let count = catalog.scan_directory(tmp.path(), SkillScope::Project);
        assert_eq!(count, 1);
    }

    #[test]
    fn test_empty_catalog() {
        let catalog = SkillCatalog::new();
        assert!(catalog.is_empty());
        assert_eq!(catalog.count(), 0);
        assert!(catalog.list_names().is_empty());
        assert!(catalog.catalog_summary().is_empty());
        assert!(catalog.match_triggers("anything").is_empty());
    }

    #[test]
    fn test_multi_scope_project_overrides_user() {
        let user_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();

        // User has a code-review skill
        create_skill_dir(
            user_dir.path(),
            "code-review",
            r#"---
name: "Code Review (User)"
description: "User-level code review"
command: "review"
---
"#,
        );

        // Project overrides with its own code-review
        create_skill_dir(
            project_dir.path(),
            "code-review",
            r#"---
name: "Code Review (Project)"
description: "Project-level code review"
command: "review"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_multi_scope(Some(user_dir.path()), Some(project_dir.path()));

        // Should have 1 skill (project overrides user)
        assert_eq!(catalog.count(), 1);
        let entry = catalog.get("code-review").unwrap();
        assert_eq!(entry.frontmatter.name, "Code Review (Project)");
        assert_eq!(entry.scope, SkillScope::Project);
    }

    #[test]
    fn test_multi_scope_user_only_when_no_project_override() {
        let user_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();

        // User has unique skill
        create_skill_dir(
            user_dir.path(),
            "my-skill",
            r#"---
name: "My User Skill"
description: "A user-specific skill"
---
"#,
        );

        // Project has a different skill
        create_skill_dir(project_dir.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_multi_scope(Some(user_dir.path()), Some(project_dir.path()));

        assert_eq!(catalog.count(), 2);
        let user_entry = catalog.get("my-skill").unwrap();
        assert_eq!(user_entry.scope, SkillScope::User);
        let proj_entry = catalog.get("code-review").unwrap();
        assert_eq!(proj_entry.scope, SkillScope::Project);
    }

    #[test]
    fn test_disabled_skill_skipped() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "disabled-skill",
            r#"---
name: "Disabled Skill"
description: "This skill is disabled"
invoke:
  mode: disabled
---
"#,
        );
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        let count = catalog.scan_directory(tmp.path(), SkillScope::Project);
        // disabled skill is skipped but still "processed" (returns Ok from load_entry)
        assert_eq!(count, 2);
        // Only the enabled skill is in the catalog
        assert_eq!(catalog.count(), 1);
        assert!(catalog.get("code-review").is_some());
        assert!(catalog.get("disabled-skill").is_none());
    }

    #[test]
    fn test_id_mismatch_warning() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "actual-dir-name",
            r#"---
id: "different-id"
name: "Mismatched Skill"
description: "ID does not match directory"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        // Skill is still loaded (just a warning)
        assert_eq!(catalog.count(), 1);
        let errors = catalog.validation_errors();
        assert!(!errors.is_empty());
        assert!(errors[0].contains("does not match directory name"));
    }

    #[test]
    fn test_alias_lookup() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "code-review",
            r#"---
name: "Code Review"
description: "Review code"
invoke:
  slash: "/review"
  aliases:
    - "/cr"
    - "/code-review"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        // Primary command works
        let entry = catalog.get_by_command("review").expect("primary command");
        assert_eq!(entry.frontmatter.name, "Code Review");

        // Aliases work
        let entry2 = catalog.get_by_command("cr").expect("alias cr");
        assert_eq!(entry2.frontmatter.name, "Code Review");

        let entry3 = catalog.get_by_command("code-review").expect("alias code-review");
        assert_eq!(entry3.frontmatter.name, "Code Review");
    }

    #[test]
    fn test_slash_conflict_produces_validation_error() {
        let tmp = TempDir::new().unwrap();
        // Two skills claiming the same slash command
        create_skill_dir(
            tmp.path(),
            "skill-a",
            r#"---
name: "Skill A"
description: "First skill"
command: "review"
---
"#,
        );
        create_skill_dir(
            tmp.path(),
            "skill-b",
            r#"---
name: "Skill B"
description: "Second skill"
command: "review"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        // Both loaded but one overwrites the command
        assert_eq!(catalog.count(), 2);
        let errors = catalog.validation_errors();
        assert!(
            errors.iter().any(|e| e.contains("conflict")),
            "Expected slash conflict warning, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_scope_field_preserved() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::User);

        let entry = catalog.get("code-review").unwrap();
        assert_eq!(entry.scope, SkillScope::User);
    }
}
