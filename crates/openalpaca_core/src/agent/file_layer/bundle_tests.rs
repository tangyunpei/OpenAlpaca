//! The file layer over the bundled one (subagent design §1.2, §2.2, §2.4,
//! §2.8): an earlier build's unedited copy no longer hides the bundled
//! template, an edited one overrides it, and every difference is said. Each
//! test works in its own temporary directory and database, never the
//! repository's `config/`, and holds the crate's home-store guard on a home
//! inside it.

use super::*;
use crate::agent::registry::{NotUsed, NotUsedReason, TemplateLayer, TemplateState};
use crate::bundled::test_support::{
    LEAD_AGENT_9B1EA1D3, LEAD_AGENT_E9F15916, SYSTEM_AGENT_2BC29A80, SYSTEM_AGENT_9ED847FE,
    SYSTEM_AGENT_EB98B820, WRITING_AGENT_2BC29A80, WRITING_AGENT_EB98B820, step_7, step_8,
};
use crate::bundled::{BUNDLED_AGENTS, ManualEdit, ShippedCopy, ShippedVersion, bundled_templates};
use crate::test_util::HomeStoreGuard;
use openalpaca_storage::SubAgentRepository;
use tempfile::TempDir;

/// A store's `config/agents/` over the bundle, as the daemon's start sets
/// it up: the bundled layer first, then the rescan.
struct Store {
    // Declared first, so it drops first.
    _guard: HomeStoreGuard,
    _dir: TempDir,
    agents: PathBuf,
    db: Database,
    registry: AgentRegistry,
    tools: ToolRegistry,
}

impl Store {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        let home = dir.path().canonicalize().unwrap().join("home");
        let registry = AgentRegistry::new();
        registry.set_bundled_layer(bundled_templates());
        Self {
            _guard: HomeStoreGuard::set(&home),
            _dir: dir,
            agents,
            db,
            registry,
            tools: ToolRegistry::default(),
        }
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.agents.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn rescan(&self, mode: LoadMode) {
        rescan_file_layer(&self.agents, mode, &self.registry, &self.tools, &self.db).unwrap();
    }

    fn layer(&self, id: &str) -> TemplateLayer {
        self.registry.provenance(id).unwrap().layer
    }

    fn not_used(&self, id: &str) -> Vec<NotUsed> {
        self.registry.provenance(id).unwrap().not_used
    }
}

/// The bundled bytes of `path` (relative to `config/`).
fn bundled(path: &str) -> &'static str {
    BUNDLED_AGENTS
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("the bundle carries {path}"))
        .contents
}

fn bundled_template(id: &str) -> AgentTemplate {
    bundled_templates()
        .into_iter()
        .find(|t| t.frontmatter.id == id)
        .unwrap()
}

fn set_aside(path: &Path, version: ShippedVersion, edit: Option<ManualEdit>) -> NotUsed {
    NotUsed {
        layer: TemplateLayer::File,
        path: Some(path.to_path_buf()),
        reason: NotUsedReason::UneditedCopy(ShippedCopy { version, edit }),
        plugin: None,
    }
}

/// §1.2, V-1: `system_agent.md` as an earlier build seeded it — with the
/// two `require_confirmation_for: ["file_delete"]` lines that switch its
/// approval prompts off — is set aside: the bundled template runs, asking
/// for its shell and file writes again, and the file stays as it was.
#[test]
fn an_earlier_builds_unedited_system_agent_no_longer_hides_the_bundled_template() {
    let store = Store::new();
    let path = store.write("system_agent.md", SYSTEM_AGENT_EB98B820);
    store.rescan(LoadMode::Start);

    let template = store.registry.get_template("system_agent").unwrap();
    assert!(template.frontmatter.require_confirmation_for.is_empty());
    let spawned = store
        .registry
        .spawn_instance("system_agent", "t1".to_string())
        .unwrap();
    assert!(spawned.constraints.require_confirmation_for.is_empty());
    assert_eq!(store.layer("system_agent"), TemplateLayer::Bundled);
    assert_eq!(
        store.not_used("system_agent"),
        [set_aside(&path, ShippedVersion::Earlier, None)]
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        SYSTEM_AGENT_EB98B820,
        "the copy is not touched"
    );
}

/// V-2: a hand copy from before seeding began (`9ed847fe`, seven months
/// before L9) is a shipped version too.
#[test]
fn a_pre_seeding_hand_copy_of_system_agent_is_set_aside_too() {
    let store = Store::new();
    let path = store.write("system_agent.md", SYSTEM_AGENT_9ED847FE);
    store.rescan(LoadMode::Start);

    assert_eq!(store.layer("system_agent"), TemplateLayer::Bundled);
    assert_eq!(
        store.not_used("system_agent"),
        [set_aside(&path, ShippedVersion::Earlier, None)]
    );
}

/// V-2a: the copy on which a person did exactly the manual's step 7 is
/// still unedited — set aside, the step named on its entry.
#[test]
fn a_system_agent_on_which_step_7_was_done_is_set_aside() {
    let store = Store::new();
    let path = store.write("system_agent.md", &step_7(SYSTEM_AGENT_2BC29A80));
    store.rescan(LoadMode::Start);

    assert_eq!(store.layer("system_agent"), TemplateLayer::Bundled);
    assert_eq!(
        store.not_used("system_agent"),
        [set_aside(
            &path,
            ShippedVersion::Earlier,
            Some(ManualEdit::Step7)
        )]
    );
}

/// V-2a: likewise step 8 on `writing_agent.md`.
#[test]
fn a_writing_agent_on_which_step_8_was_done_is_set_aside() {
    let store = Store::new();
    let path = store.write("writing_agent.md", &step_8(WRITING_AGENT_2BC29A80));
    store.rescan(LoadMode::Start);

    assert_eq!(store.layer("writing_agent"), TemplateLayer::Bundled);
    assert_eq!(
        store.not_used("writing_agent"),
        [set_aside(
            &path,
            ShippedVersion::Earlier,
            Some(ManualEdit::Step8)
        )]
    );
}

/// §1.2: an earlier build's unedited `writing_agent.md`, without
/// `file_read`, gives way to the bundled template, which reads files
/// (storage design §15) — no hand step.
#[test]
fn an_earlier_builds_unedited_writing_agent_gives_way_to_one_that_reads_files() {
    let store = Store::new();
    assert!(!WRITING_AGENT_EB98B820.contains("\"file_read\""));
    store.write("writing_agent.md", WRITING_AGENT_EB98B820);
    store.rescan(LoadMode::Start);

    let template = store.registry.get_template("writing_agent").unwrap();
    assert!(
        template
            .frontmatter
            .capabilities
            .iter()
            .any(|c| c == "file_read")
    );
}

/// V-2b: a store seeded before TEMPLATE-TIMEOUT holds `9b1ea1d3`'s
/// `lead_agent.md` (an earlier version), one first started after it
/// `e9f15916`'s (the current one): both are set aside and the bundled lead
/// runs.
#[test]
fn a_copy_seeded_on_either_side_of_template_timeout_is_set_aside() {
    for (text, version) in [
        (LEAD_AGENT_9B1EA1D3, ShippedVersion::Earlier),
        (LEAD_AGENT_E9F15916, ShippedVersion::Current),
    ] {
        let store = Store::new();
        let path = store.write("lead_agent.md", text);
        store.rescan(LoadMode::Start);

        assert_eq!(store.layer("lead_agent"), TemplateLayer::Bundled);
        assert_eq!(
            store.not_used("lead_agent"),
            [set_aside(&path, version, None)]
        );
        let lead = store.registry.get_template("lead_agent").unwrap();
        assert!(same_definition(&lead, &bundled_template("lead_agent")));
    }
}

/// §1.2, U2: a copy the person edited — one byte is enough — is theirs: it
/// overrides the bundled template of its id, is listed so, and nothing
/// writes it, at the start or at a reload.
#[test]
fn an_edited_copy_overrides_the_bundled_template_and_is_kept_byte_for_byte() {
    let store = Store::new();
    let edited =
        bundled("agents/system_agent.md").replace("max_tool_calls: 30", "max_tool_calls: 31");
    assert_ne!(edited, bundled("agents/system_agent.md"));
    let path = store.write("system_agent.md", &edited);
    store.rescan(LoadMode::Start);
    store.rescan(LoadMode::Reload);

    let template = store.registry.get_template("system_agent").unwrap();
    assert_eq!(template.frontmatter.max_tool_calls, Some(31));
    let provenance = store.registry.provenance("system_agent").unwrap();
    assert_eq!(provenance.layer, TemplateLayer::File);
    assert_eq!(provenance.path, Some(path.clone()));
    assert!(provenance.overrides_bundled());
    assert_eq!(
        provenance.not_used,
        [NotUsed {
            layer: TemplateLayer::Bundled,
            path: None,
            reason: NotUsedReason::Overridden,
            plugin: None,
        }]
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
}

/// U5: deleting an override brings the bundled template back at the next
/// rescan — the watcher's event — with its idle instance and its row.
#[test]
fn deleting_an_override_brings_the_bundled_template_back_without_a_restart() {
    let store = Store::new();
    let edited =
        bundled("agents/system_agent.md").replace("max_tool_calls: 30", "max_tool_calls: 31");
    let path = store.write("system_agent.md", &edited);
    store.rescan(LoadMode::Start);
    assert_eq!(
        store
            .registry
            .get("system_agent")
            .unwrap()
            .constraints
            .max_tool_calls,
        Some(31)
    );

    std::fs::remove_file(&path).unwrap();
    store.rescan(LoadMode::Reload);

    let template = store.registry.get_template("system_agent").unwrap();
    assert_eq!(template.frontmatter.max_tool_calls, Some(30));
    assert_eq!(store.layer("system_agent"), TemplateLayer::Bundled);
    assert!(store.not_used("system_agent").is_empty());
    let idle = store
        .registry
        .get("system_agent")
        .expect("its idle instance");
    assert_eq!(idle.constraints.max_tool_calls, Some(30));
    let row = SubAgentRepository::new(&store.db)
        .get("system_agent")
        .unwrap()
        .unwrap();
    let constraints: serde_json::Value =
        serde_json::from_str(row.constraints_json.as_deref().unwrap()).unwrap();
    assert_eq!(
        constraints["max_tool_calls"], 30,
        "the row is the bundled one's"
    );
}

/// L-1a, U5: an override of `lead_agent.md` whose `model:` is edited, then
/// deleted — after each rescan the singleton lead's next spawn runs on the
/// model in force: the override's, then the bundled template's.
#[test]
fn editing_then_deleting_a_lead_agent_override_moves_the_next_spawns_model() {
    let store = Store::new();
    let bundled_model = bundled_template("lead_agent").frontmatter.model;
    let edited = bundled("agents/lead_agent.md").replace(
        &format!("model: \"{}\"", bundled_model.as_deref().unwrap()),
        "model: \"model-override\"",
    );
    assert_ne!(edited, bundled("agents/lead_agent.md"));
    let path = store.write("lead_agent.md", &edited);
    store.rescan(LoadMode::Start);

    let first = store
        .registry
        .spawn_instance("lead_agent", "t1".to_string())
        .unwrap();
    assert_eq!(first.llm_config.model.as_deref(), Some("model-override"));
    store.registry.destroy_instance(&first.id);

    std::fs::remove_file(&path).unwrap();
    store.rescan(LoadMode::Reload);

    let next = store
        .registry
        .spawn_instance("lead_agent", "t2".to_string())
        .unwrap();
    assert_eq!(next.llm_config.model, bundled_model);
}

/// V-4: a copy renamed but not edited is still unedited — matched by
/// content, whatever the file is called.
#[test]
fn a_renamed_unedited_copy_is_still_set_aside() {
    let store = Store::new();
    let path = store.write("system_agent copy.md", bundled("agents/system_agent.md"));
    store.rescan(LoadMode::Start);

    assert_eq!(store.layer("system_agent"), TemplateLayer::Bundled);
    assert_eq!(
        store.not_used("system_agent"),
        [set_aside(&path, ShippedVersion::Current, None)]
    );
}

/// L-4, U2: a file that does not parse, naming a bundled id, withholds it —
/// a spawn is refused naming the file — and the bundled template does not
/// stand in for the person's broken edit.
#[test]
fn a_broken_override_of_a_bundled_id_withholds_it_rather_than_falling_back() {
    let store = Store::new();
    let path = store.write(
        "system_agent.md",
        "---\nid: system_agent\ndescription: half edited\n---\n",
    );
    store.rescan(LoadMode::Start);

    let err = store
        .registry
        .spawn_instance("system_agent", "t1".to_string())
        .unwrap_err();
    assert!(
        err.starts_with(&format!(
            "template 'system_agent' is withheld: {} does not parse",
            path.display()
        )) && err.ends_with("; fix it, or remove it to use the bundled template"),
        "{err}"
    );
    assert!(store.registry.get_template("system_agent").is_none());
    assert!(
        !store
            .registry
            .list_templates()
            .iter()
            .any(|t| t.frontmatter.id == "system_agent")
    );
    assert!(matches!(
        store.registry.provenance("system_agent").unwrap().state,
        TemplateState::Withheld { .. }
    ));
}

/// W-1: an edited copy that keeps the two `file_delete` lines (the step-7
/// trap, edited) runs as the person's file — and is warned about, at load,
/// naming what the list does. Nothing is refused or rewritten.
#[test]
#[tracing_test::traced_test]
fn a_confirmation_list_naming_no_tool_is_warned() {
    let store = Store::new();
    for tool in crate::tools::builtins::builtin_tools(None, None, None) {
        store.tools.register(tool).unwrap();
    }
    let edited = SYSTEM_AGENT_2BC29A80.replace("max_tool_calls: 30", "max_tool_calls: 31");
    let path = store.write("system_agent.md", &edited);
    store.rescan(LoadMode::Start);

    let warning = format!(
        "{}: require_confirmation_for names \"file_delete\", which is no tool; this list \
         replaces the default prompts, so this template's file_write, file_edit and \
         shell_execute run without asking",
        path.display()
    );
    let provenance = store.registry.provenance("system_agent").unwrap();
    assert_eq!(provenance.layer, TemplateLayer::File);
    assert!(logs_contain(&warning));
    assert_eq!(provenance.warnings, [warning]);
    assert_eq!(
        store
            .registry
            .get_template("system_agent")
            .unwrap()
            .frontmatter
            .require_confirmation_for,
        ["file_delete"],
        "the file runs as written"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
}

/// W-1 leaves a plugin's tool (`<plugin>::<tool>`) unchecked: at start the
/// plugins register their tools after the templates load, so its absence
/// is no typo — while `file_delete` beside it is still warned about.
#[test]
fn a_confirmation_list_naming_a_plugin_tool_is_not_warned_about_it() {
    let store = Store::new();
    for tool in crate::tools::builtins::builtin_tools(None, None, None) {
        store.tools.register(tool).unwrap();
    }
    let path = store.write(
        "mine.md",
        "---\nid: mine\nname: Mine\ndescription: mine\ncapabilities:\n  - \"shell_execute\"\n\
         require_confirmation_for:\n  - \"myplugin::send\"\n  - \"file_delete\"\n---\n",
    );
    store.rescan(LoadMode::Start);

    assert_eq!(
        store.registry.provenance("mine").unwrap().warnings,
        [format!(
            "{}: require_confirmation_for names \"file_delete\", which is no tool; this list \
             replaces the default prompts, so this template's shell_execute runs without asking",
            path.display()
        )]
    );
}

/// §2.11: a row a DELETE archived is set back when a template of its id
/// runs again — an earlier build's DELETE of a shipped template, at this
/// build's start, where the bundled template runs; and this build's of a
/// person's own file, when the file comes back. While nothing runs under
/// the id the row stays archived.
#[test]
fn an_archived_row_is_set_back_when_a_template_of_its_id_runs_again() {
    let store = Store::new();
    let repo = SubAgentRepository::new(&store.db);
    let status = |id: &str| repo.get(id).unwrap().unwrap().status;
    store.rescan(LoadMode::Start);
    // As an earlier build's DELETE /v1/agent-templates/research_agent left
    // it: the seeded file moved to .archived/, the row archived.
    repo.update_status("research_agent", "archived", None)
        .unwrap();

    store.rescan(LoadMode::Start);

    assert_eq!(store.layer("research_agent"), TemplateLayer::Bundled);
    assert_eq!(status("research_agent"), "idle");

    let reader = "---\nid: reader\nname: Reader\ndescription: reads\ncapabilities:\n  - \
                  \"file_read\"\n---\n";
    let path = store.write("reader.md", reader);
    store.rescan(LoadMode::Reload);
    std::fs::remove_file(&path).unwrap();
    repo.update_status("reader", "archived", None).unwrap();
    store.rescan(LoadMode::Reload);
    assert_eq!(status("reader"), "archived", "nothing runs under the id");

    store.write("reader.md", reader);
    store.rescan(LoadMode::Reload);

    assert_eq!(status("reader"), "idle");
}

/// §1.3: the start says what runs — how many templates are bundled, which
/// files override one, which unedited copies are set aside and which of
/// them are earlier versions — and names each override on its own line.
#[test]
#[tracing_test::traced_test]
fn the_start_log_names_every_set_aside_copy_and_override() {
    let store = Store::new();
    store.write("system_agent.md", SYSTEM_AGENT_EB98B820);
    store.write("writing_agent.md", WRITING_AGENT_EB98B820);
    for file in BUNDLED_AGENTS {
        let name = file.path.strip_prefix("agents/").unwrap();
        if !["system_agent.md", "writing_agent.md", "code_agent.md"].contains(&name) {
            store.write(name, file.contents);
        }
    }
    let code = store.write(
        "code_agent.md",
        &format!("{}\nMy own note.\n", bundled("agents/code_agent.md")),
    );
    store.rescan(LoadMode::Start);

    assert!(logs_contain(&format!(
        "Agent templates: 9 bundled; 1 from {}, overriding a bundled template (code_agent); \
         8 unedited copies of shipped versions set aside, not read (2 of them earlier \
         versions: system_agent.md, writing_agent.md)",
        store.agents.display()
    )));
    assert!(logs_contain(&format!(
        "Agent template {} overrides the bundled template code_agent",
        code.display()
    )));

    // A fresh store's `agents/`, created empty: the bundle alone.
    assert_eq!(
        start_summary(9, &store.agents, 0, &[], &[]),
        format!(
            "Agent templates: 9 bundled; 0 from {}; no unedited copy of a shipped version set aside",
            store.agents.display()
        )
    );
}
