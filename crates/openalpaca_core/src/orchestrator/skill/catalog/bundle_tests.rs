//! The config directory's `skills/` over the bundled skills (subagent design
//! §1.2, §2.2 V-5, §2.4 L-2, L-2a, L-5, §2.6 K-3–K-5, §2.8 W-2): an earlier
//! build's unedited copy no longer hides the bundled skill, an edited one
//! overrides it, a disabled or broken one holds its key, and the pack the
//! bundled skills run from is there when they run. Each test works in its
//! own temporary directory — a store's `config/skills/` and
//! `state/cache/bundled-skills/` — never the repository's `config/`, and
//! holds the crate's home-store guard on a home inside it.

use super::*;
use crate::bundled::{BUNDLED_SKILLS, ShippedVersion};
use crate::test_util::HomeStoreGuard;
use tempfile::TempDir;

/// A store as the daemon's start sets it up: the pack written and the
/// bundled layer set, then the config directory's `skills/` read.
struct Store {
    // Declared first, so it drops first.
    _guard: HomeStoreGuard,
    _dir: TempDir,
    pack: PathBuf,
    skills: PathBuf,
    catalog: Arc<SkillCatalog>,
}

impl Store {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // Canonical, as the daemon's script path is (`ScriptToolBuiltIn::new`).
        let root = dir.path().canonicalize().unwrap();
        let skills = root.join("config/skills");
        std::fs::create_dir_all(&skills).unwrap();
        Self {
            _guard: HomeStoreGuard::set(&root.join("home")),
            _dir: dir,
            pack: root.join("state/cache/bundled-skills"),
            skills,
            catalog: Arc::new(SkillCatalog::new()),
        }
    }

    fn start(&self) -> usize {
        assert_eq!(self.catalog.load_bundled_layer(&self.pack), 4);
        self.catalog.scan_config_layer(&self.skills)
    }

    /// `82022776`'s `create-skill` — what that build's seeding wrote — as
    /// `<config>/skills/create-skill/`.
    fn seed_82022776_create_skill(&self) -> PathBuf {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/bundled/fixtures/82022776/skills/create-skill");
        let folder = self.skills.join("create-skill");
        for name in [
            "SKILL.md",
            "scripts/check_exists.sh",
            "scripts/list_skills.sh",
            "scripts/scaffold.sh",
            "scripts/write_skill_md.sh",
        ] {
            let target = folder.join(name);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::copy(fixture.join(name), &target).unwrap();
        }
        folder
    }

    /// This build's copy of the bundled skill `key`, as a seeding would
    /// have written it.
    fn copy_current(&self, key: &str) -> PathBuf {
        let folder = self.skills.join(key);
        let prefix = format!("skills/{key}/");
        for file in BUNDLED_SKILLS
            .iter()
            .filter(|f| f.path.starts_with(&prefix))
        {
            let target = folder.join(file.path.strip_prefix(&prefix).unwrap());
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, file.contents).unwrap();
        }
        folder
    }

    fn entry(&self, key: &str) -> SkillEntry {
        self.catalog
            .get(key)
            .unwrap_or_else(|| panic!("no skill under {key}"))
    }

    fn runs_bundled(&self, key: &str) -> bool {
        let entry = self.entry(key);
        entry.scope == SkillScope::Bundled
            && entry
                .skill_md_path
                .as_deref()
                .is_some_and(|p| p.starts_with(&self.pack))
    }
}

fn edit_skill_md(folder: &Path) {
    let path = folder.join("SKILL.md");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replacen("description:", "# mine\ndescription:", 1),
    )
    .unwrap();
}

/// §9 C4: a store `82022776` seeded keeps that build's `create-skill`,
/// whose `user` scope wrote where nothing reads. It is set aside at load;
/// the bundled `create-skill` runs from the pack, its scripts told the
/// config directory's `skills/` by the daemon, so a scaffolded skill lands
/// there and a `user` scope is refused naming it. Nothing is written in the
/// pack, and the seeded folder is not touched.
#[cfg(unix)]
#[tokio::test]
async fn an_earlier_builds_create_skill_is_set_aside_and_the_bundled_one_writes_into_config_skills()
{
    use crate::tools::builtins::ScriptToolBuiltIn;
    use crate::tools::registry::BuiltInTool;

    let store = Store::new();
    let seeded = store.seed_82022776_create_skill();
    let before: Vec<Vec<u8>> = ["SKILL.md", "scripts/scaffold.sh"]
        .iter()
        .map(|name| std::fs::read(seeded.join(name)).unwrap())
        .collect();

    assert_eq!(
        store.start(),
        0,
        "no folder of the config directory is read"
    );
    assert!(store.runs_bundled("create-skill"));
    assert_eq!(
        store.catalog.provenance("create-skill").not_used,
        vec![SkillNotUsed {
            source: "file",
            path: Some(seeded.clone()),
            reason: "unedited_copy",
            version: Some(ShippedVersion::Earlier),
        }]
    );

    let entry = store.entry("create-skill");
    let doc = store.catalog.load_full("create-skill").unwrap();
    let script = |name: &str| {
        let cfg = doc
            .frontmatter
            .scripts
            .iter()
            .find(|cfg| cfg.name == name)
            .unwrap();
        ScriptToolBuiltIn::new(
            entry.skill_dir.as_deref().unwrap(),
            cfg,
            store.catalog.skill_roots(),
        )
        .unwrap()
    };

    let refused = script("check_exists")
        .execute(&serde_json::json!({"id": "hello-world", "scope": "user"}))
        .await
        .unwrap_err();
    assert!(
        refused.contains(&store.skills.display().to_string()),
        "the refusal names the config directory's skills/: {refused}"
    );

    script("scaffold")
        .execute(&serde_json::json!({"id": "hello-world"}))
        .await
        .unwrap();
    assert!(store.skills.join("hello-world").is_dir());
    assert!(!store.pack.join("hello-world").exists());
    let after: Vec<Vec<u8>> = ["SKILL.md", "scripts/scaffold.sh"]
        .iter()
        .map(|name| std::fs::read(seeded.join(name)).unwrap())
        .collect();
    assert_eq!(before, after, "the seeded folder is not touched");
}

/// §1.2: an override whose `SKILL.md` says `invoke.mode: disabled` holds the
/// key with nothing runnable — the bundled skill is hidden, not shown
/// through — and says so at start.
#[tracing_test::traced_test]
#[test]
fn a_disabled_override_hides_the_bundled_skill() {
    let store = Store::new();
    let folder = store.copy_current("code-review");
    let path = folder.join("SKILL.md");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replacen("mode: auto", "mode: disabled", 1)).unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("mode: disabled")
    );

    store.start();

    assert!(store.catalog.get("code-review").is_none());
    assert!(store.catalog.get_by_command("review").is_none());
    assert!(
        !store
            .catalog
            .list_names()
            .contains(&"code-review".to_string())
    );
    assert!(logs_contain(&format!(
        "skill 'code-review' is disabled by {}; the bundled one is not loaded",
        path.display()
    )));
}

/// L-4 for skills: an override whose `SKILL.md` does not parse holds its key
/// — never the bundled skill in its place — and the diagnostics name it.
#[test]
fn a_broken_override_holds_its_key() {
    let store = Store::new();
    let folder = store.skills.join("explain-code");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("SKILL.md"), "no frontmatter here\n").unwrap();

    store.start();

    assert!(store.catalog.get("explain-code").is_none());
    let diagnostics = store.catalog.diagnostics();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].skill_id.as_deref(), Some("explain-code"));
    assert_eq!(diagnostics[0].severity, SkillDiagnosticSeverity::Error);
    // The other three bundled skills run.
    for key in ["code-review", "commit-message", "create-skill"] {
        assert!(store.runs_bundled(key), "{key}");
    }
}

/// V-5: Finder writes `.DS_Store` into a folder merely opened; that does not
/// make a seeded copy the person's.
#[test]
fn a_ds_store_does_not_make_a_seeded_skill_folder_yours() {
    let store = Store::new();
    let folder = store.seed_82022776_create_skill();
    std::fs::write(folder.join(".DS_Store"), [0u8, 1, 2]).unwrap();
    std::fs::write(folder.join("scripts/.DS_Store"), [3u8]).unwrap();

    store.start();

    assert!(store.runs_bundled("create-skill"));
    assert_eq!(
        store.catalog.provenance("create-skill").not_used[0].reason,
        "unedited_copy"
    );
}

/// V-5, no completeness clause: a seeded copy missing a file is still an
/// unedited copy, and the bundled skill runs with all its files.
#[test]
fn an_unedited_copy_missing_a_file_the_bundle_later_gained_is_still_ignored() {
    let store = Store::new();
    let folder = store.seed_82022776_create_skill();
    std::fs::remove_file(folder.join("scripts/list_skills.sh")).unwrap();

    store.start();

    assert!(store.runs_bundled("create-skill"));
    assert!(
        store
            .pack
            .join("create-skill/scripts/list_skills.sh")
            .is_file()
    );
}

/// V-5: a file added where no build shipped one makes the folder the
/// person's — it overrides the bundled skill, and the route says so.
#[test]
fn a_seeded_skill_folder_with_an_added_file_is_yours() {
    let store = Store::new();
    let folder = store.copy_current("commit-message");
    std::fs::write(folder.join("notes.md"), "mine").unwrap();

    assert_eq!(store.start(), 1);

    let entry = store.entry("commit-message");
    assert_eq!(entry.scope, SkillScope::Project);
    assert_eq!(entry.skill_dir.as_deref(), Some(folder.as_path()));
    let provenance = store.catalog.provenance("commit-message");
    assert!(provenance.overrides_bundled);
    assert_eq!(
        provenance.not_used,
        vec![SkillNotUsed {
            source: "bundled",
            path: Some(store.pack.join("commit-message")),
            reason: "overridden",
            version: None,
        }]
    );
}

/// L-2a: a config folder created while the daemon runs, over a bundled key,
/// is the config directory's — `Project`, overriding — never the bundled
/// entry's scope carried over.
#[test]
fn a_new_config_skill_over_a_bundled_key_is_project_scoped() {
    let store = Store::new();
    store.start();
    assert!(store.runs_bundled("explain-code"));

    let folder = store.copy_current("explain-code");
    edit_skill_md(&folder);
    store.catalog.reload_skill(&folder).unwrap();

    let entry = store.entry("explain-code");
    assert_eq!(entry.scope, SkillScope::Project);
    assert_eq!(entry.skill_dir.as_deref(), Some(folder.as_path()));
    assert!(store.catalog.provenance("explain-code").overrides_bundled);
}

/// L-2a outside the layers: reloading a folder of another directory over a
/// key a different directory's skill holds does not carry that skill's
/// scope.
#[test]
fn a_reload_carries_a_scope_only_from_the_same_directory() {
    let store = Store::new();
    let user = store.skills.parent().unwrap().join("user-skills");
    let project = store.skills.parent().unwrap().join("project-skills");
    for root in [&user, &project] {
        let folder = root.join("notes");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("SKILL.md"),
            "---\nname: Notes\ndescription: d\nrequires_capabilities: [file_read]\n---\n\nBody.\n",
        )
        .unwrap();
    }
    store.catalog.scan_directory(&user, SkillScope::User);
    assert_eq!(store.entry("notes").scope, SkillScope::User);

    store.catalog.reload_skill(&project.join("notes")).unwrap();
    assert_eq!(store.entry("notes").scope, SkillScope::Project);
    store.catalog.reload_skill(&project.join("notes")).unwrap();
    assert_eq!(store.entry("notes").scope, SkillScope::Project);
}

/// L-5, U5: deleting an override folder brings the bundled skill back at
/// once, with its slash command; deleting a set-aside copy changes nothing.
#[test]
fn deleting_an_override_folder_brings_the_bundled_skill_back_live() {
    let store = Store::new();
    let override_folder = store.copy_current("code-review");
    edit_skill_md(&override_folder);
    let copy = store.copy_current("explain-code");
    store.start();
    assert!(!store.runs_bundled("code-review"));
    assert!(store.runs_bundled("explain-code"));

    std::fs::remove_dir_all(&override_folder).unwrap();
    store.catalog.reload_skill(&override_folder).unwrap();
    assert!(store.runs_bundled("code-review"));
    assert!(store.catalog.get_by_command("review").is_some());
    assert_eq!(
        store.catalog.provenance("code-review"),
        SkillProvenance::default()
    );

    std::fs::remove_dir_all(&copy).unwrap();
    store.catalog.reload_skill(&copy).unwrap();
    assert!(store.runs_bundled("explain-code"));
    assert!(store.catalog.provenance("explain-code").not_used.is_empty());
}

/// K-3: a person deleted `state/cache/bundled-skills/` while the daemon
/// ran; the next bundled skill to run finds its pack written again.
#[test]
fn a_deleted_pack_is_rewritten_before_a_bundled_skill_runs() {
    let store = Store::new();
    store.start();
    std::fs::remove_dir_all(&store.pack).unwrap();

    let doc = store.catalog.load_full("create-skill").unwrap();

    assert_eq!(doc.frontmatter.name, "Create Skill");
    assert!(
        store
            .pack
            .join("create-skill/scripts/scaffold.sh")
            .is_file()
    );
    assert!(store.pack.join("code-review/SKILL.md").is_file());
}

/// K-4: a pack that cannot be written leaves the bundled layer empty — no
/// bundled skill runs, none half-written — with one `WARN`; the scripts are
/// not told a pack.
#[tracing_test::traced_test]
#[test]
fn a_pack_that_cannot_be_written_leaves_no_bundled_skill_and_warns() {
    let store = Store::new();
    let cache = store.pack.parent().unwrap();
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    std::fs::write(cache, "a file where state/cache/ should be").unwrap();

    assert_eq!(store.catalog.load_bundled_layer(&store.pack), 0);
    store.catalog.scan_config_layer(&store.skills);

    assert_eq!(store.catalog.count(), 0);
    assert_eq!(store.catalog.skill_roots().bundled, None);
    assert!(logs_contain(&format!(
        "the built-in skills could not be written to {}",
        store.pack.display()
    )));
    assert!(logs_contain("no built-in skill is loaded"));
}

/// W-2: an override whose `SKILL.md` is edited but whose `scaffold.sh` is
/// still `82022776`'s — the script that wrote `user`-scope skills where
/// nothing reads — runs the old file, and the diagnostics say so.
#[test]
fn an_override_holding_an_earlier_shipped_script_is_warned() {
    let store = Store::new();
    let folder = store.seed_82022776_create_skill();
    edit_skill_md(&folder);

    store.start();

    assert_eq!(store.entry("create-skill").scope, SkillScope::Project);
    let stale: Vec<SkillDiagnostic> = store
        .catalog
        .diagnostics()
        .into_iter()
        .filter(|d| d.code == SkillDiagnosticCode::StaleShippedFile)
        .collect();
    let paths: Vec<&str> = stale.iter().map(|d| d.path.as_str()).collect();
    let scaffold = folder.join("scripts/scaffold.sh");
    assert!(
        paths.contains(&scaffold.display().to_string().as_str()),
        "{paths:?}"
    );
    let diagnostic = stale
        .iter()
        .find(|d| d.path == scaffold.display().to_string())
        .unwrap();
    assert_eq!(diagnostic.severity, SkillDiagnosticSeverity::Warning);
    assert_eq!(diagnostic.skill_id.as_deref(), Some("create-skill"));
    assert_eq!(
        diagnostic.message,
        format!(
            "{} is the version an earlier build shipped; the built-in create-skill's differs, and this override runs the old file",
            scaffold.display()
        )
    );
    // Every one of the four old scripts is named; the edited SKILL.md is
    // the person's, not stale.
    assert_eq!(stale.len(), 4, "{paths:?}");
}

/// K-5 when seeding failed: a config directory's `skills/` that is absent
/// at start is still the root the scripts are told — never left unnamed, so
/// a script never falls back to writing into the pack — and its absence is
/// one diagnostic. The bundled skills run.
#[test]
fn an_absent_config_skills_directory_is_still_the_scripts_root() {
    let store = Store::new();
    std::fs::remove_dir(&store.skills).unwrap();

    assert_eq!(store.start(), 0);

    assert_eq!(
        store.catalog.skill_roots().config.as_deref(),
        Some(store.skills.as_path())
    );
    let diagnostics = store.catalog.diagnostics();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code, SkillDiagnosticCode::ReadFailed);
    assert_eq!(diagnostics[0].path, store.skills.display().to_string());
    for key in [
        "code-review",
        "commit-message",
        "create-skill",
        "explain-code",
    ] {
        assert!(store.runs_bundled(key), "{key}");
    }
}

/// W-2 at every reload: a hot edit that breaks a loaded override's
/// `SKILL.md` names each old script once, however often it is saved —
/// the reload works W-2 out from the folder again, never adds to what the
/// last one found.
#[test]
fn a_broken_hot_edit_of_a_stale_override_names_each_old_script_once() {
    let store = Store::new();
    let folder = store.seed_82022776_create_skill();
    edit_skill_md(&folder);
    store.start();

    let counts = || {
        let diagnostics = store.catalog.diagnostics();
        let stale = diagnostics
            .iter()
            .filter(|d| d.code == SkillDiagnosticCode::StaleShippedFile)
            .count();
        let errors = diagnostics
            .iter()
            .filter(|d| d.severity == SkillDiagnosticSeverity::Error)
            .count();
        (stale, errors)
    };
    assert_eq!(counts(), (4, 0));

    std::fs::write(folder.join("SKILL.md"), "no frontmatter here\n").unwrap();
    for _ in 0..2 {
        assert!(store.catalog.reload_skill(&folder).is_err());
        assert_eq!(counts(), (4, 1));
        // The loaded override stays listed, failing to run until fixed.
        assert_eq!(store.entry("create-skill").scope, SkillScope::Project);
    }
}

/// §1.3's start line, with a store an earlier build seeded whole: the four
/// skills set aside, one of them (`create-skill`) an earlier version.
#[test]
fn the_start_line_names_the_pack_and_every_set_aside_copy() {
    let store = Store::new();
    store.seed_82022776_create_skill();
    for key in ["code-review", "commit-message", "explain-code"] {
        store.copy_current(key);
    }
    store.start();

    assert_eq!(
        store.catalog.start_summary(),
        format!(
            "Skills: 4 bundled (run from {}); 0 from {}; 4 unedited copies of shipped versions set aside, not read (1 of them an earlier version: create-skill)",
            store.pack.display(),
            store.skills.display()
        )
    );
    assert!(store.catalog.override_lines().is_empty());

    let mine = store.copy_current("code-review");
    edit_skill_md(&mine);
    store.catalog.reload_skill(&mine).unwrap();
    assert_eq!(
        store.catalog.start_summary(),
        format!(
            "Skills: 4 bundled (run from {}); 1 from {}, overriding a bundled skill (code-review); 3 unedited copies of shipped versions set aside, not read (1 of them an earlier version: create-skill)",
            store.pack.display(),
            store.skills.display()
        )
    );
    assert_eq!(
        store.catalog.override_lines(),
        vec![format!(
            "Skill {} overrides the bundled skill code-review",
            mine.display()
        )]
    );
}

/// P-3, unchanged until B-05: a plugin's skill goes over a bundled one; when
/// it goes, the bundled skill is back on the key, and no tombstone answers
/// for it.
#[test]
fn a_plugin_skill_over_a_bundled_key_gives_it_back_when_it_goes() {
    struct Stub;
    #[async_trait::async_trait]
    impl openalpaca_api::plugin_traits::PluginSkillExecutor for Stub {
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
            "code-review"
        }
    }
    let store = Store::new();
    store.start();
    let frontmatter = SkillFrontmatter {
        name: "Plugin review".to_string(),
        description: "d".to_string(),
        ..Default::default()
    };
    store.catalog.register_plugin_skill(
        "code-review".to_string(),
        frontmatter,
        Arc::new(Stub),
        "p".to_string(),
    );
    assert!(matches!(
        store.entry("code-review").source,
        SkillSource::Plugin { .. }
    ));

    store.catalog.remove_plugin_skill("code-review", "p");

    assert!(store.runs_bundled("code-review"));
    assert!(store.catalog.tombstone("code-review").is_none());
}

/// §2.12 D-2: what `openalpaca skills tidy` lists is what the catalog sets
/// aside — in file-name order — and not an override, nor a folder of the
/// person's own.
#[test]
fn set_aside_skill_folders_names_exactly_what_the_catalog_sets_aside() {
    let store = Store::new();
    let current = store.copy_current("code-review");
    let earlier = store.seed_82022776_create_skill();
    edit_skill_md(&store.copy_current("explain-code"));
    std::fs::create_dir_all(store.skills.join("daily-digest")).unwrap();
    std::fs::write(
        store.skills.join("daily-digest/SKILL.md"),
        "---\nname: daily-digest\ndescription: A digest\n---\nDigest.\n",
    )
    .unwrap();
    store.start();

    assert_eq!(
        bundled::set_aside_skill_folders(&store.skills).unwrap(),
        [
            (current.clone(), ShippedVersion::Current),
            (earlier.clone(), ShippedVersion::Earlier)
        ]
    );
    for (key, folder, version) in [
        ("code-review", current, ShippedVersion::Current),
        ("create-skill", earlier, ShippedVersion::Earlier),
    ] {
        assert_eq!(
            store.catalog.provenance(key).not_used,
            vec![SkillNotUsed {
                source: "file",
                path: Some(folder),
                reason: "unedited_copy",
                version: Some(version),
            }]
        );
    }
    assert!(
        bundled::set_aside_skill_folders(&store.skills.join("absent"))
            .unwrap()
            .is_empty()
    );
}
