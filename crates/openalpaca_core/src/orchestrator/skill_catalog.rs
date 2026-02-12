//! Skill Catalog: progressive loading of SKILL.md-based skills.
//!
//! Level 1 (startup): Scans `config/skills/` and loads only YAML frontmatter
//! for each SKILL.md file, building a lightweight in-memory catalog.
//!
//! Level 2 (on-demand): Loads the full SKILL.md body when a skill is invoked,
//! providing the complete instructions and templates.

use crate::middleware::skill::{
    SkillDocument, SkillFrontmatter, parse_skill_frontmatter, parse_skill_markdown,
};
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
}

/// Central skill catalog — lightweight at startup, loads full content on demand.
///
/// Thread-safe via internal `RwLock`. All mutations go through `&self` methods.
pub struct SkillCatalog {
    /// All known skills, keyed by normalized skill name (lowercase).
    entries: RwLock<HashMap<String, SkillEntry>>,
    /// Slash command index: maps command (e.g. "review") → normalized skill name.
    command_index: RwLock<HashMap<String, String>>,
}

impl SkillCatalog {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            command_index: RwLock::new(HashMap::new()),
        }
    }

    /// Scan a directory for skill folders containing SKILL.md.
    ///
    /// Each direct child directory that contains a `SKILL.md` file is treated
    /// as a skill. Only the YAML frontmatter is parsed (Level 1).
    /// Returns the count of skills successfully loaded.
    pub fn scan_directory(&self, dir: &Path) -> usize {
        let read_dir = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) => {
                tracing::warn!("SkillCatalog: failed to read skills directory {}: {}", dir.display(), e);
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

            match self.load_entry(&skill_md, &child_path) {
                Ok(()) => count += 1,
                Err(e) => {
                    tracing::warn!(
                        "SkillCatalog: failed to load {}: {}",
                        skill_md.display(),
                        e
                    );
                }
            }
        }

        count
    }

    /// Load a single SKILL.md entry (Level 1 — frontmatter only).
    fn load_entry(&self, skill_md: &Path, skill_dir: &Path) -> Result<(), String> {
        let content = std::fs::read_to_string(skill_md)
            .map_err(|e| format!("read error: {}", e))?;

        let frontmatter = parse_skill_frontmatter(&content)
            .map_err(|e| format!("parse error: {}", e))?;

        // Compile trigger patterns
        let compiled_triggers: Vec<Regex> = frontmatter
            .trigger_patterns
            .iter()
            .filter_map(|pattern| {
                match Regex::new(&format!("(?i){}", pattern)) {
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
                }
            })
            .collect();

        let key = frontmatter.name.to_lowercase();

        // Register command mapping
        if let Some(ref cmd) = frontmatter.command {
            if let Ok(mut idx) = self.command_index.write() {
                idx.insert(cmd.to_lowercase(), key.clone());
            }
        }

        let entry = SkillEntry {
            frontmatter,
            skill_md_path: skill_md.to_path_buf(),
            skill_dir: skill_dir.to_path_buf(),
            compiled_triggers,
        };

        if let Ok(mut entries) = self.entries.write() {
            entries.insert(key, entry);
        }

        Ok(())
    }

    /// Look up a skill by name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<SkillEntry> {
        let guard = self.entries.read().ok()?;
        guard.get(&name.to_lowercase()).cloned()
    }

    /// Look up a skill by slash command (e.g. "review" → SkillEntry).
    pub fn get_by_command(&self, command: &str) -> Option<SkillEntry> {
        let cmd_lower = command.to_lowercase();
        let skill_name = {
            let guard = self.command_index.read().ok()?;
            guard.get(&cmd_lower).cloned()?
        };
        self.get(&skill_name)
    }

    /// Find skills whose trigger_patterns match the given text.
    ///
    /// Returns matched skill names, ordered by specificity (most patterns matched first).
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

    /// List all registered skill names.
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
                            e.frontmatter.command.clone(),
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
        let entry = self.get(name)
            .ok_or_else(|| format!("Skill '{}' not found in catalog", name))?;

        let content = std::fs::read_to_string(&entry.skill_md_path)
            .map_err(|e| format!("Failed to read {}: {}", entry.skill_md_path.display(), e))?;

        parse_skill_markdown(&content)
            .map_err(|e| format!("Failed to parse {}: {}", entry.skill_md_path.display(), e))
    }

    /// Remove a skill from the catalog.
    pub fn remove(&self, name: &str) {
        let key = name.to_lowercase();
        // Remove command index entry
        if let Ok(entries) = self.entries.read() {
            if let Some(entry) = entries.get(&key) {
                if let Some(ref cmd) = entry.frontmatter.command {
                    if let Ok(mut idx) = self.command_index.write() {
                        idx.remove(&cmd.to_lowercase());
                    }
                }
            }
        }
        // Remove the entry
        if let Ok(mut entries) = self.entries.write() {
            entries.remove(&key);
        }
    }

    /// Hot-reload a single skill directory.
    ///
    /// Removes the old entry (if any) and re-scans the directory.
    pub fn reload_skill(&self, skill_dir: &Path) -> Result<(), String> {
        let skill_md = skill_dir.join("SKILL.md");
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

        // Read frontmatter to get the name for removal of old entry
        let content = std::fs::read_to_string(&skill_md)
            .map_err(|e| format!("read error: {}", e))?;
        let fm = parse_skill_frontmatter(&content)
            .map_err(|e| format!("parse error: {}", e))?;

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

        // Re-load
        self.load_entry(&skill_md, skill_dir)?;
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
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
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
        let count = catalog.scan_directory(tmp.path());
        assert_eq!(count, 2);
        assert_eq!(catalog.count(), 2);
    }

    #[test]
    fn test_get_by_name() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path());

        let entry = catalog.get("Code Review").expect("should find by name");
        assert_eq!(entry.frontmatter.name, "Code Review");
        assert_eq!(entry.frontmatter.command, Some("review".to_string()));

        // Case insensitive
        let entry2 = catalog.get("code review").expect("should find case-insensitive");
        assert_eq!(entry2.frontmatter.name, "Code Review");

        assert!(catalog.get("nonexistent").is_none());
    }

    #[test]
    fn test_get_by_command() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);
        create_skill_dir(tmp.path(), "explain-code", EXPLAIN_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path());

        let entry = catalog.get_by_command("review").expect("should find /review");
        assert_eq!(entry.frontmatter.name, "Code Review");

        let entry2 = catalog.get_by_command("explain-code").expect("should find /explain-code");
        assert_eq!(entry2.frontmatter.name, "Explain Code");

        assert!(catalog.get_by_command("nonexistent").is_none());
    }

    #[test]
    fn test_match_triggers() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);
        create_skill_dir(tmp.path(), "explain-code", EXPLAIN_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path());

        let matches = catalog.match_triggers("please review my code");
        assert!(!matches.is_empty());
        assert!(matches.contains(&"code review".to_string()));

        let matches2 = catalog.match_triggers("explain this code to me");
        assert!(!matches2.is_empty());
        assert!(matches2.contains(&"explain code".to_string()));

        let matches3 = catalog.match_triggers("hello world");
        assert!(matches3.is_empty());
    }

    #[test]
    fn test_load_full() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path());

        let doc = catalog.load_full("Code Review").expect("should load full");
        assert_eq!(doc.frontmatter.name, "Code Review");
        assert!(doc.body.contains("Analyze the code"));
        assert!(doc.sections.contains_key("Instructions"));
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
        catalog.scan_directory(tmp.path());

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
        catalog.scan_directory(tmp.path());
        assert_eq!(catalog.count(), 1);
        assert!(catalog.get_by_command("review").is_some());

        catalog.remove("Code Review");
        assert_eq!(catalog.count(), 0);
        assert!(catalog.get_by_command("review").is_none());
    }

    #[test]
    fn test_reload_skill() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path());

        let entry = catalog.get("Code Review").unwrap();
        assert_eq!(entry.frontmatter.description, "Review code for bugs and style issues");

        // Overwrite with updated content
        let updated = REVIEW_SKILL.replace(
            "Review code for bugs and style issues",
            "Updated description",
        );
        std::fs::write(skill_dir.join("SKILL.md"), updated).unwrap();

        catalog.reload_skill(&skill_dir).expect("reload should succeed");

        let entry2 = catalog.get("Code Review").unwrap();
        assert_eq!(entry2.frontmatter.description, "Updated description");
    }

    #[test]
    fn test_scan_ignores_non_directories() {
        let tmp = TempDir::new().unwrap();
        // Create a regular file (not a directory) in the skills dir
        std::fs::write(tmp.path().join("not_a_dir.md"), "hello").unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        let count = catalog.scan_directory(tmp.path());
        assert_eq!(count, 1);
    }

    #[test]
    fn test_scan_ignores_dirs_without_skill_md() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("empty-dir")).unwrap();
        create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

        let catalog = SkillCatalog::new();
        let count = catalog.scan_directory(tmp.path());
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
}
