//! A file that switches a template off (subagent design §2.4 L-4a, §7 O-1
//! (b)): `disabled: true` in a top-level `.md` of `agents/` takes the id it
//! names out of the catalog, refuses a spawn of it naming the file, keeps
//! every other layer's template of the id out, and deleting the file brings
//! the template back. Each test works in its own temporary directory and
//! database, never the repository's `config/`, and holds the crate's
//! home-store guard on a home inside it.

use super::*;
use crate::agent::AgentSource;
use crate::bundled::{BUNDLED_AGENTS, bundled_templates};
use crate::runner::lead_agent::{
    build_lead_agent_prompt_from_templates, spawn_subagent_tool_definition_from_templates,
    spawn_subagents_batch_tool_definition,
};
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

    fn ids(&self) -> Vec<String> {
        self.registry
            .list_templates()
            .into_iter()
            .map(|t| t.frontmatter.id)
            .collect()
    }

    fn row_status(&self, id: &str) -> Option<String> {
        SubAgentRepository::new(&self.db)
            .get(id)
            .unwrap()
            .map(|row| row.status)
    }
}

/// The bundled bytes of `config/<path>`.
fn bundled(path: &str) -> &'static str {
    BUNDLED_AGENTS
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("the bundle carries {path}"))
        .contents
}

/// The bundled `research_agent.md` with `disabled: <value>` added to its
/// frontmatter: a full template that says it.
fn full_research_agent(value: &str) -> String {
    let text = bundled("agents/research_agent.md");
    let anchor = "id: \"research_agent\"\n";
    assert_eq!(text.matches(anchor).count(), 1);
    text.replace(anchor, &format!("{anchor}disabled: {value}\n"))
}

/// L-4a's refusal, as a spawn of a switched-off id meets it.
fn refusal(id: &str, path: &Path) -> String {
    format!(
        "template '{id}' is switched off by {} (disabled: true); delete that file, or remove the \
         line, to use it again",
        path.display()
    )
}

/// L-4a: the two-line form — `id:` and `disabled: true`, nothing else —
/// switches the bundled template off: it leaves the list, and a spawn of it
/// is refused naming the file. The file is not touched.
#[test]
fn a_file_saying_disabled_switches_a_bundled_template_off() {
    let store = Store::new();
    let switch = "---\nid: research_agent\ndisabled: true\n---\n";
    let path = store.write("research_agent.md", switch);
    store.rescan(LoadMode::Start);

    assert!(!store.ids().contains(&"research_agent".to_string()));
    assert!(store.registry.get_template("research_agent").is_none());
    let err = store
        .registry
        .spawn_instance("research_agent", "t1".to_string())
        .unwrap_err();
    assert_eq!(err, refusal("research_agent", &path));
    assert_eq!(
        store.registry.provenance("research_agent").unwrap().path,
        Some(path.clone())
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), switch);
    assert_eq!(store.ids().len(), 8, "the other eight still run");
}

/// L-4a: a full template that says `disabled: true` is switched off whole —
/// its other keys are not read, so not even the start's check of its
/// `annotation:` names stops the daemon over it.
#[test]
fn a_full_template_saying_disabled_is_switched_off_whole() {
    let store = Store::new();
    let text = full_research_agent("true").replace(
        "  - \"read_result\"\n",
        "  - \"read_result\"\n  - \"annotation:no_such_hint\"\n",
    );
    let path = store.write("research_agent.md", &text);
    store.rescan(LoadMode::Start);

    assert!(store.registry.get_template("research_agent").is_none());
    let err = store
        .registry
        .spawn_instance("research_agent", "t1".to_string())
        .unwrap_err();
    assert_eq!(err, refusal("research_agent", &path));
}

/// L-4a: what the lead is offered leaves a switched-off id out — the
/// `<agents>` block of its prompt and both spawn tools' descriptions, all
/// built from the resolved list, as `run_lead_agent` builds them.
#[test]
fn a_switched_off_id_leaves_the_leads_agents_block_and_both_spawn_descriptions() {
    let store = Store::new();
    store.write("research_agent.md", &full_research_agent("true"));
    store.rescan(LoadMode::Start);

    let workers: Vec<AgentTemplate> = store
        .registry
        .list_templates()
        .into_iter()
        .filter(|t| t.frontmatter.id != "lead_agent")
        .collect();
    let prompt = build_lead_agent_prompt_from_templates(
        &crate::compose::ComposeEngine::new(16),
        "",
        &workers,
        200_000,
        None,
        None,
        None,
        8,
    );
    let single = spawn_subagent_tool_definition_from_templates(&workers);
    let batch = spawn_subagents_batch_tool_definition(&workers);

    assert!(
        prompt.contains("writing_agent"),
        "the block lists the others"
    );
    assert!(!prompt.contains("research_agent"), "{prompt}");
    for definition in [single, batch] {
        let text = serde_json::to_string(&definition).unwrap();
        assert!(text.contains("writing_agent"), "{text}");
        assert!(!text.contains("research_agent"), "{text}");
    }
}

/// U5: deleting the switching file brings the bundled template back at the
/// next rescan — the watcher's event — with no restart.
#[test]
fn deleting_the_switching_file_brings_the_template_back_without_a_restart() {
    let store = Store::new();
    let path = store.write("research_agent.md", &full_research_agent("true"));
    store.rescan(LoadMode::Start);
    assert!(store.registry.get_template("research_agent").is_none());

    std::fs::remove_file(&path).unwrap();
    store.rescan(LoadMode::Reload);

    let back = store.registry.get_template("research_agent").unwrap();
    assert_eq!(
        back.frontmatter.description,
        bundled_templates()
            .into_iter()
            .find(|t| t.frontmatter.id == "research_agent")
            .unwrap()
            .frontmatter
            .description
    );
    assert!(
        store
            .registry
            .spawn_instance("research_agent", "t1".to_string())
            .is_ok()
    );
}

/// A plugin's agent executor that is never driven: the test only registers.
struct IdleExecutor;

#[async_trait::async_trait]
impl openalpaca_api::plugin_traits::PluginAgentExecutor for IdleExecutor {
    async fn spawn(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &serde_json::Value,
    ) -> Result<bool, String> {
        Ok(true)
    }
    async fn step(
        &self,
        _: &str,
        _: Option<&serde_json::Value>,
    ) -> Result<(String, String, Vec<serde_json::Value>), String> {
        Ok(("complete".to_string(), String::new(), vec![]))
    }
    async fn stop(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn plugin_id(&self) -> &str {
        "acme"
    }
    fn agent_id(&self) -> &str {
        "research_agent"
    }
}

/// P-2 with L-4a: a bundled id switched off stays held — a plugin's
/// template of it is refused, and the refusal is listed on the id's row —
/// so neither the bundled template nor a plugin's shows through.
#[test]
fn a_plugin_template_cannot_take_a_switched_off_bundled_id() {
    let store = Store::new();
    let path = store.write("research_agent.md", &full_research_agent("true"));
    store.rescan(LoadMode::Start);

    let mut plugin = parse_agent_markdown(
        "---\nid: research_agent\nname: Acme research\ndescription: the plugin's\n---\n",
    )
    .unwrap();
    plugin.source = AgentSource::Plugin {
        plugin_id: "acme".to_string(),
        executor: std::sync::Arc::new(IdleExecutor),
    };
    assert!(!store.registry.register_template(plugin));

    assert!(store.registry.get_template("research_agent").is_none());
    let err = store
        .registry
        .spawn_instance("research_agent", "t1".to_string())
        .unwrap_err();
    assert_eq!(err, refusal("research_agent", &path));
    let not_used = store
        .registry
        .provenance("research_agent")
        .unwrap()
        .not_used;
    assert!(
        not_used.iter().any(
            |n| n.reason == NotUsedReason::PluginRefused && n.plugin.as_deref() == Some("acme")
        ),
        "{not_used:?}"
    );
}

/// L-1a and L-4a: the switched-off id's idle instance goes — `agents
/// config` and `?skill=` no longer find it — and its `agent` row is kept,
/// reading `disabled`; when the file goes, the instance and an `idle` row
/// are back.
#[test]
fn a_switched_off_template_loses_its_idle_instance_and_its_row_reads_disabled() {
    let store = Store::new();
    store.rescan(LoadMode::Start);
    assert!(store.registry.get_with_version("research_agent").is_some());
    assert_eq!(store.row_status("research_agent").as_deref(), Some("idle"));

    let path = store.write("research_agent.md", &full_research_agent("true"));
    store.rescan(LoadMode::Reload);

    assert!(store.registry.get_with_version("research_agent").is_none());
    assert!(
        store
            .registry
            .find_by_capability("web_access")
            .iter()
            .all(|agent| agent.template_id != "research_agent")
    );
    assert_eq!(
        store.row_status("research_agent").as_deref(),
        Some("disabled")
    );

    std::fs::remove_file(&path).unwrap();
    store.rescan(LoadMode::Reload);

    assert!(store.registry.get_with_version("research_agent").is_some());
    assert_eq!(store.row_status("research_agent").as_deref(), Some("idle"));
}

/// L-4a: of a switching file and a template of the same id, the first by
/// file name holds the id (the duplicate rule); a file that does not parse
/// never takes a switched-off id.
#[test]
#[tracing_test::traced_test]
fn a_switching_file_and_a_template_of_one_id_follow_the_duplicate_rule() {
    let store = Store::new();
    let own = "---\nid: reader\nname: Reader\ndescription: reads\ncapabilities:\n  - \
               \"file_read\"\n---\n";
    let switch = store.write("a_off.md", "---\nid: reader\ndisabled: yes\n---\n");
    let template = store.write("b_reader.md", own);
    store.write("c_broken.md", "---\nid: reader\n");
    store.rescan(LoadMode::Start);

    assert!(store.registry.get_template("reader").is_none());
    assert_eq!(
        store
            .registry
            .spawn_instance("reader", "t1".to_string())
            .unwrap_err(),
        refusal("reader", &switch)
    );
    assert!(logs_contain(&format!(
        "Agent templates {} and {} both claim the id reader; {} is used (the first by file name)",
        switch.display(),
        template.display(),
        switch.display()
    )));

    std::fs::rename(&switch, store.agents.join("z_off.md")).unwrap();
    store.rescan(LoadMode::Reload);

    assert_eq!(
        store.registry.provenance("reader").unwrap().path,
        Some(template),
        "the template sorts first now"
    );
    assert!(store.registry.get_template("reader").is_some());
}

/// §1.3: the start names each file that switches a template off, on a line
/// of its own (`INFO`, deliberate, like an override), after the summary and
/// the override lines.
#[test]
#[tracing_test::traced_test]
fn the_start_log_names_a_switching_file() {
    let store = Store::new();
    let path = store.write(
        "research_agent.md",
        "---\nid: research_agent\ndisabled: true\n---\n",
    );
    let own = store.write("own_off.md", "---\nid: reader\ndisabled: \"true\"\n---\n");
    let code = store.write(
        "code_agent.md",
        &bundled("agents/code_agent.md").replace("Code Agent", "My Code Agent"),
    );
    store.rescan(LoadMode::Start);

    let switched = format!(
        "Agent template {} switches off the bundled template research_agent (disabled: true)",
        path.display()
    );
    let own_switched = format!(
        "Agent template {} switches off the template reader (disabled: true)",
        own.display()
    );
    let overrides = format!(
        "Agent template {} overrides the bundled template code_agent",
        code.display()
    );
    assert!(logs_contain(&switched));
    assert!(logs_contain(&own_switched));
    logs_assert(|lines: &[&str]| {
        let at = |text: &str| {
            lines
                .iter()
                .position(|line| line.contains(text))
                .ok_or_else(|| format!("no line contains {text}"))
        };
        let summary = at("Agent templates: ")?;
        let override_line = at(&overrides)?;
        let first_switch = at(&switched)?.min(at(&own_switched)?);
        if summary < override_line && override_line < first_switch {
            Ok(())
        } else {
            Err(format!(
                "§1.3's order: summary {summary}, override {override_line}, switched off \
                 {first_switch}"
            ))
        }
    });
}

/// L-4a: a switching file removed sets its id's row back to `idle` only
/// when a template runs under the id again; a row a `DELETE` archived, of
/// an id under which nothing runs, is not made to read `idle`.
#[test]
fn a_switching_file_gone_leaves_no_idle_row_for_an_id_with_no_template() {
    let store = Store::new();
    let own = store.write(
        "reader.md",
        "---\nid: reader\nname: Reader\ndescription: reads\ncapabilities:\n  - \"file_read\"\n---\n",
    );
    store.rescan(LoadMode::Start);
    assert_eq!(store.row_status("reader").as_deref(), Some("idle"));
    std::fs::remove_file(&own).unwrap();
    SubAgentRepository::new(&store.db)
        .update_status("reader", "archived", None)
        .unwrap();
    store.rescan(LoadMode::Reload);

    let off = store.write("reader_off.md", "---\nid: reader\ndisabled: true\n---\n");
    store.rescan(LoadMode::Reload);
    assert_eq!(store.row_status("reader").as_deref(), Some("disabled"));

    std::fs::remove_file(&off).unwrap();
    store.rescan(LoadMode::Reload);

    assert!(store.registry.get_template("reader").is_none());
    assert_ne!(store.row_status("reader").as_deref(), Some("idle"));
}

/// `disabled: false`, or no key at all, is today's template: read whole, an
/// override of the bundled one like any edited copy.
#[test]
fn disabled_false_is_todays_template() {
    let store = Store::new();
    let path = store.write("research_agent.md", &full_research_agent("false"));
    store.rescan(LoadMode::Start);

    let template = store.registry.get_template("research_agent").unwrap();
    assert_eq!(template.frontmatter.name, "Research Agent");
    let provenance = store.registry.provenance("research_agent").unwrap();
    assert_eq!(provenance.path, Some(path));
    assert!(provenance.overrides_bundled());
    assert!(
        store
            .registry
            .spawn_instance("research_agent", "t1".to_string())
            .is_ok()
    );
}
