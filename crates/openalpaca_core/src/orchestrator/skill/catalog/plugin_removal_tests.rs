//! N-PLUG-6: a plugin's unload removes only the catalog entry its own load
//! registered — by its exact id, when that entry's source is this plugin —
//! and never reaches a file skill through `remove`'s display-name scan.

use super::*;
use crate::middleware::skill::SkillScope;
use tempfile::TempDir;

struct StubExecutor;

#[async_trait::async_trait]
impl openalpaca_api::plugin_traits::PluginSkillExecutor for StubExecutor {
    async fn invoke(
        &self,
        _query: &str,
        _context: &serde_json::Value,
        _tool_executor: &dyn openalpaca_api::plugin_traits::ToolCallbackExecutor,
    ) -> Result<String, String> {
        Ok(String::new())
    }
    fn plugin_id(&self) -> &str {
        "p"
    }
    fn skill_id(&self) -> &str {
        "triage"
    }
}

fn register(catalog: &SkillCatalog, id: &str, slash: &str, plugin: &str) {
    let mut frontmatter = SkillFrontmatter {
        name: id.to_string(),
        description: format!("{id} via {plugin}"),
        ..Default::default()
    };
    frontmatter.invoke.slash = Some(slash.to_string());
    catalog.register_plugin_skill(
        id.to_string(),
        frontmatter,
        Arc::new(StubExecutor),
        plugin.to_string(),
    );
}

/// A file skill directory `<dir>/SKILL.md` named `name`, slash `slash`.
fn file_skill(root: &Path, dir: &str, name: &str, slash: &str) {
    let skill = root.join(dir);
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        format!(
            "---\nname: \"{name}\"\ndescription: \"a file skill\"\ninvoke:\n  slash: \"{slash}\"\n---\n\n## Instructions\n\nDo it.\n"
        ),
    )
    .unwrap();
}

fn is_file_based(catalog: &SkillCatalog, id: &str) -> bool {
    catalog
        .entries_snapshot()
        .into_iter()
        .any(|(key, entry)| key == id && matches!(entry.source, SkillSource::FileBased))
}

/// The name scan: with no entry keyed `review`, the plugin's removal of
/// `review` used to find the file skill whose display name is `Review` and
/// delete it. Now nothing is removed and nothing is attributed to the plugin.
#[test]
fn a_plugin_removal_never_reaches_a_file_skill_through_the_name_scan() {
    let tmp = TempDir::new().unwrap();
    file_skill(tmp.path(), "code-review", "Review", "/code-review");
    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    catalog.remove_plugin_skill("review", "p");

    assert!(
        is_file_based(&catalog, "code-review"),
        "the file skill named like the id survives"
    );
    assert!(catalog.get_by_command("code-review").is_some());
    assert!(catalog.tombstone("review").is_none());
    assert!(catalog.tombstone("code-review").is_none());
}

/// By id: a hot-reloaded file skill took the plugin skill's key. The
/// plugin's removal of that key leaves the file skill — its source is not
/// this plugin — and leaves no tombstone over its commands.
#[test]
fn a_plugin_removal_leaves_a_file_skill_that_took_its_key() {
    let tmp = TempDir::new().unwrap();
    let catalog = SkillCatalog::new();
    register(&catalog, "triage", "/triage", "p");
    file_skill(tmp.path(), "triage", "Triage", "/triage-file");
    catalog.scan_directory(tmp.path(), SkillScope::Project);
    assert!(is_file_based(&catalog, "triage"));

    catalog.remove_plugin_skill("triage", "p");

    assert!(
        is_file_based(&catalog, "triage"),
        "the file skill on the key survives"
    );
    assert!(catalog.get_by_command("triage-file").is_some());
    assert!(catalog.tombstone("triage").is_none());
    assert!(catalog.tombstone("triage-file").is_none());
}

/// Another plugin's skill on the key is not this plugin's to remove.
#[test]
fn a_plugin_removal_leaves_another_plugins_skill() {
    let catalog = SkillCatalog::new();
    register(&catalog, "triage", "/triage", "p");
    register(&catalog, "triage", "/triage", "q");

    catalog.remove_plugin_skill("triage", "p");

    let entry = catalog
        .get("triage")
        .expect("q's skill survives p's unload");
    assert!(matches!(
        entry.source,
        SkillSource::Plugin { ref plugin_id, .. } if plugin_id == "q"
    ));
    assert!(catalog.get_by_command("triage").is_some());
    assert!(catalog.tombstone("triage").is_none());
}

/// The plugin's own entry goes, its commands with it, and the tombstone
/// names it — what T2 always did for what the plugin owns.
#[test]
fn a_plugin_removal_takes_its_own_entry_and_tombstones_it() {
    let catalog = SkillCatalog::new();
    register(&catalog, "Triage", "/triage", "p");

    catalog.remove_plugin_skill("TRIAGE", "p");

    assert!(catalog.get("triage").is_none());
    assert!(catalog.get_by_command("triage").is_none());
    for key in ["triage", "/triage"] {
        assert_eq!(
            catalog.tombstone(key).map(|t| t.plugin_id).as_deref(),
            Some("p"),
            "{key}"
        );
    }
}
