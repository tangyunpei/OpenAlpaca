//! `GET /v1/skills` — the skill catalog's shape (plan Phase 8 item 2, GAP-18's
//! remaining half).
//!
//! Same two axes as `/v1/tools`, read the same way: a row's `origin` is a
//! **read** of the extension ledger, never a switch, and a file skill — which
//! is on no ENABLE axis at all — carries no enable field of any kind.

use super::*;

use std::path::Path;

use openalpaca_core::middleware::skill::SkillScope;
use openalpaca_core::orchestrator::skill_catalog::SkillCatalog;
use openalpaca_core::tools::extensions::{ExtensionId, ExtensionLedger, ExtensionState};

/// A file skill, written to disk and scanned the way the daemon scans
/// `config/skills/` at boot — so the row is built from real parsed frontmatter,
/// not from a hand-made struct.
fn scan_file_skill(catalog: &SkillCatalog, dir: &Path, id: &str, body: &str) {
    let skill_dir = dir.join(id);
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(skill_dir.join("SKILL.md"), body).expect("SKILL.md");
    catalog.scan_directory(dir, SkillScope::Project);
}

struct SkillBridge {
    plugin: &'static str,
    skill: &'static str,
}

#[async_trait::async_trait]
impl openalpaca_api::plugin_traits::PluginSkillExecutor for SkillBridge {
    async fn invoke(
        &self,
        _query: &str,
        _context: &serde_json::Value,
        _tool_executor: &dyn openalpaca_api::plugin_traits::ToolCallbackExecutor,
    ) -> Result<String, String> {
        Ok(String::new())
    }
    fn plugin_id(&self) -> &str {
        self.plugin
    }
    fn skill_id(&self) -> &str {
        self.skill
    }
}

fn register_plugin_skill(catalog: &SkillCatalog, plugin: &'static str, skill: &'static str) {
    let frontmatter = openalpaca_core::middleware::skill::parse_skill_frontmatter(&format!(
        "---\n\
         id: {skill}\n\
         name: Daily Digest\n\
         version: 3.1.0\n\
         description: Summarise the day\n\
         invoke:\n  \
           mode: scheduled\n  \
           slash: /digest\n  \
           cron: \"0 9 * * *\"\n\
         routing:\n  \
           keywords:\n    \
             - digest\n\
         requires_capabilities:\n  \
           - notion::create_page\n\
         ---\n\nbody\n"
    ))
    .expect("frontmatter parses");
    catalog.register_plugin_skill(
        skill.to_string(),
        frontmatter,
        std::sync::Arc::new(SkillBridge { plugin, skill }),
        plugin.to_string(),
    );
}

fn find<'a>(rows: &'a [serde_json::Value], id: &str) -> &'a serde_json::Value {
    rows.iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("'{id}' is not in the catalog"))
}

const REVIEW_SKILL: &str = "---\n\
     id: code-review\n\
     name: Code Review\n\
     version: 0.1.0\n\
     description: Review code for bugs\n\
     invoke:\n  \
       mode: auto\n  \
       slash: /review\n\
     routing:\n  \
       keywords:\n    \
         - review\n    \
         - bugs\n\
     requires_capabilities:\n  \
       - file_read\n\
     ---\n\nInstructions.\n";

/// A file skill is on no ENABLE axis, so its `origin` is `null` and its row
/// carries no enable field at all — the same rule a builtin tool row follows.
/// Every other field is the frontmatter's, verbatim.
#[test]
fn a_file_skill_row_has_no_origin_and_no_enable_field() {
    let dir = tempfile::tempdir().expect("tempdir");
    let catalog = SkillCatalog::new();
    scan_file_skill(&catalog, dir.path(), "code-review", REVIEW_SKILL);
    let ledger = ExtensionLedger::new();

    let rows = skills_json(&catalog, &ledger, &HashMap::new());
    let row = find(&rows, "code-review");
    assert_eq!(row["source"], "file");
    assert_eq!(row["origin"], serde_json::Value::Null);
    for absent in ["enabled", "state", "denied", "plugin_id"] {
        assert!(
            row.get(absent).is_none(),
            "a file-skill row must not carry '{absent}': {row}"
        );
    }
    assert_eq!(row["name"], "Code Review");
    assert_eq!(row["description"], "Review code for bugs");
    assert_eq!(row["version"], "0.1.0");
    assert_eq!(row["author"], "file:project");
    assert_eq!(row["requires_capabilities"], serde_json::json!(["file_read"]));
    assert_eq!(
        row["triggers"],
        serde_json::json!({"slash": "review", "keywords": ["review", "bugs"]}),
        "the slash trigger is served without its leading '/', the way the \
         command index keys it"
    );
    assert_eq!(
        row["schedule"],
        serde_json::Value::Null,
        "no invoke.cron means no schedule"
    );
    assert_eq!(row["invocations_today"], 0);
}

/// A plugin skill mirrors the tools rule: `origin` is a read of the ledger,
/// taken at render time, keyed by the plugin's **directory** name.
#[test]
fn a_plugin_skill_rows_origin_tracks_the_ledger() {
    let catalog = SkillCatalog::new();
    register_plugin_skill(&catalog, "notion", "daily-digest");
    let ledger = ExtensionLedger::new();
    let ext = ExtensionId::plugin("notion");
    ledger.upsert(&ext, true, ExtensionState::Enabled);

    let rows = skills_json(&catalog, &ledger, &HashMap::new());
    let row = find(&rows, "daily-digest");
    assert_eq!(row["source"], "plugin");
    assert_eq!(
        row["origin"],
        serde_json::json!({
            "kind": "plugin", "id": "notion", "enabled": true, "state": "enabled"
        })
    );
    assert_eq!(row["author"], "plugin:notion");
    assert_eq!(row["name"], "Daily Digest");
    assert_eq!(row["version"], "3.1.0");
    assert_eq!(
        row["requires_capabilities"],
        serde_json::json!(["notion::create_page"])
    );
    assert_eq!(row["schedule"], "0 9 * * *", "invoke.cron is the schedule");
    assert_eq!(
        row["triggers"],
        serde_json::json!({"slash": "digest", "keywords": ["digest"]})
    );

    // The ledger moves; the row follows, with no re-registration.
    ledger.upsert(&ext, false, ExtensionState::Disabled);
    let rows = skills_json(&catalog, &ledger, &HashMap::new());
    let row = find(&rows, "daily-digest");
    assert_eq!(row["origin"]["enabled"], false);
    assert_eq!(row["origin"]["state"], "disabled");
}

/// §6.2a, the same fail-open the tool catalog and the gate take: no ledger
/// entry means *"no supervisor owns this yet"*, not *"disabled"*.
#[test]
fn an_unrecorded_plugins_skill_reads_as_enabled() {
    let catalog = SkillCatalog::new();
    register_plugin_skill(&catalog, "unrecorded", "daily-digest");

    let rows = skills_json(&catalog, &ExtensionLedger::new(), &HashMap::new());
    let row = find(&rows, "daily-digest");
    assert_eq!(row["origin"]["enabled"], true);
    assert_eq!(row["origin"]["state"], "enabled");
}

/// **The row shape, exactly** — eleven keys and no twelfth, on both sources.
#[test]
fn every_row_carries_the_eleven_keys_and_no_others() {
    let dir = tempfile::tempdir().expect("tempdir");
    let catalog = SkillCatalog::new();
    scan_file_skill(&catalog, dir.path(), "code-review", REVIEW_SKILL);
    register_plugin_skill(&catalog, "notion", "daily-digest");

    let mut expected = [
        "id",
        "name",
        "description",
        "source",
        "origin",
        "requires_capabilities",
        "triggers",
        "schedule",
        "invocations_today",
        "version",
        "author",
    ];
    expected.sort_unstable();

    let rows = skills_json(&catalog, &ExtensionLedger::new(), &HashMap::new());
    assert_eq!(rows.len(), 2);
    for row in &rows {
        let object = row.as_object().expect("each row is an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, expected, "row is not the agreed shape: {row}");
        assert!(row["id"].is_string());
        assert!(row["name"].is_string());
        assert!(row["description"].is_string());
        assert!(row["requires_capabilities"].is_array());
        assert!(row["triggers"]["keywords"].is_array());
        assert!(row["invocations_today"].is_i64());
        assert!(row["author"].is_string());
        assert!(
            ["file", "plugin"].contains(&row["source"].as_str().unwrap()),
            "unknown source: {row}"
        );
        assert_eq!(
            row["origin"].is_null(),
            row["source"] == "file",
            "the origin-null rule does not hold for {row}"
        );
    }
}

/// `HashMap` iteration jitters, so the array is sorted by id and two reads of
/// an unchanged catalog are byte-identical.
#[test]
fn the_catalog_is_sorted_by_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let catalog = SkillCatalog::new();
    for id in ["zzz-last", "aaa-first", "mmm-middle"] {
        scan_file_skill(
            &catalog,
            dir.path(),
            id,
            &REVIEW_SKILL.replace("id: code-review", &format!("id: {id}")),
        );
    }

    let ledger = ExtensionLedger::new();
    let rows = skills_json(&catalog, &ledger, &HashMap::new());
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "the catalog must be sorted");
    assert_eq!(
        rows,
        skills_json(&catalog, &ledger, &HashMap::new()),
        "two reads of an unchanged catalog must agree"
    );
}

/// `invocations_today` is the count for **that skill id**, and `0` — not
/// `null` — for a skill nobody has run today.
#[test]
fn invocations_today_is_per_id_and_defaults_to_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let catalog = SkillCatalog::new();
    scan_file_skill(&catalog, dir.path(), "code-review", REVIEW_SKILL);
    register_plugin_skill(&catalog, "notion", "daily-digest");

    let counts = HashMap::from([("code-review".to_string(), 7i64)]);
    let rows = skills_json(&catalog, &ExtensionLedger::new(), &counts);
    assert_eq!(find(&rows, "code-review")["invocations_today"], 7);
    assert_eq!(find(&rows, "daily-digest")["invocations_today"], 0);
}

/// **The log does not key on the catalog id.** Every invocation path resolves
/// the entry and then passes `entry.frontmatter.name` on as the `skill_id`
/// written to `skill_execution_log` — `/slash` and router selection through
/// `Intent::SkillInvocation` (`intent/skill_match.rs:31,55` →
/// `skill/invocation.rs:140`), and the model's `invoke_skill` tool through
/// `builtins/invoke_skill.rs:173`. So `invocations_today` resolves a logged key
/// the way `SkillCatalog::get` does — lowercased, id first, then frontmatter
/// name — or it would report 0 for every file skill anyone actually ran.
#[test]
fn invocations_today_counts_rows_logged_under_the_frontmatter_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let catalog = SkillCatalog::new();
    scan_file_skill(&catalog, dir.path(), "code-review", REVIEW_SKILL);

    // "Code Review" is the frontmatter name; "code-review" is the id. Both
    // spellings reach the column, and both belong to this one skill.
    let counts = HashMap::from([
        ("Code Review".to_string(), 5i64),
        ("code-review".to_string(), 2i64),
    ]);
    let rows = skills_json(&catalog, &ExtensionLedger::new(), &counts);
    assert_eq!(
        find(&rows, "code-review")["invocations_today"],
        7,
        "both spellings are the same skill and both count"
    );
}

/// A logged key that resolves to no catalog entry is simply not counted —
/// never attached to some other row. (The GUI still shows that health row; it
/// is the *catalog* listing that has nothing to say about it.)
#[test]
fn a_logged_key_no_skill_claims_is_counted_for_nobody() {
    let dir = tempfile::tempdir().expect("tempdir");
    let catalog = SkillCatalog::new();
    scan_file_skill(&catalog, dir.path(), "code-review", REVIEW_SKILL);

    let counts = HashMap::from([("deleted-skill".to_string(), 9i64)]);
    let rows = skills_json(&catalog, &ExtensionLedger::new(), &counts);
    assert_eq!(rows.len(), 1);
    assert_eq!(find(&rows, "code-review")["invocations_today"], 0);
}
