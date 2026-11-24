//! AGENT-SETS (item 22 of the owner's second answers of 2026-11-14): a set
//! (`PUT /v1/agents/{id}/config`, `openalpaca agents set`) never changes a
//! running agent; it is stored and loaded for every new run of its
//! template, singleton or not, and across a restart. Each test works in its
//! own temporary directory and database and holds the crate's home-store
//! guard on a home inside it.

use super::*;
use crate::agent::config::AgentLlmConfigFile;
use crate::agent::file_layer::{LoadMode, rescan_file_layer};
use crate::agent::subagent::{AgentStatus, SubAgent};
use crate::test_util::HomeStoreGuard;
use crate::tools::ToolRegistry;
use tempfile::TempDir;

struct Store {
    // Declared first, so it drops first.
    _guard: HomeStoreGuard,
    dir: TempDir,
    db: Database,
    tools: ToolRegistry,
}

/// One daemon's agents over the store: its registry, filled as the start
/// fills it, and the service the routes call.
struct Daemon {
    registry: Arc<AgentRegistry>,
    service: AgentConfigService,
}

impl Store {
    fn new(files: &[(&str, String)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        for (name, contents) in files {
            std::fs::write(agents.join(name), contents).unwrap();
        }
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        let home = dir.path().canonicalize().unwrap().join("home");
        Self {
            _guard: HomeStoreGuard::set(&home),
            dir,
            db,
            tools: ToolRegistry::default(),
        }
    }

    fn agents(&self) -> PathBuf {
        self.dir.path().join("agents")
    }

    /// A daemon's start on this store: the bundled layer, then the files.
    fn start(&self) -> Daemon {
        let registry = Arc::new(AgentRegistry::new());
        registry.set_bundled_layer(crate::bundled::bundled_templates());
        rescan_file_layer(
            &self.agents(),
            LoadMode::Start,
            &registry,
            &self.tools,
            &self.db,
        )
        .unwrap();
        let service = AgentConfigService::new(registry.clone(), self.agents(), self.db.clone());
        Daemon { registry, service }
    }

    /// A template file rewritten while the daemon runs: the watcher's rescan.
    fn rewrite(&self, daemon: &Daemon, name: &str, contents: &str) {
        std::fs::write(self.agents().join(name), contents).unwrap();
        rescan_file_layer(
            &self.agents(),
            LoadMode::Reload,
            &daemon.registry,
            &self.tools,
            &self.db,
        )
        .unwrap();
    }

    fn remove(&self, daemon: &Daemon, name: &str) {
        std::fs::remove_file(self.agents().join(name)).unwrap();
        rescan_file_layer(
            &self.agents(),
            LoadMode::Reload,
            &daemon.registry,
            &self.tools,
            &self.db,
        )
        .unwrap();
    }

    /// The model `GET /v1/agents/{id}` reports: the row's.
    fn row_model(&self, id: &str) -> Option<String> {
        let row = SubAgentRepository::new(&self.db).get(id).unwrap().unwrap();
        let llm: serde_json::Value =
            serde_json::from_str(row.llm_config_json.as_deref().unwrap_or("null")).unwrap();
        llm.get("model")
            .and_then(|m| m.as_str())
            .map(str::to_string)
    }
}

impl Daemon {
    /// `openalpaca agents set <id> llm.model <model>`: read, change one
    /// key, put back with the version read.
    fn set_model(&self, id: &str, model: &str) -> Result<u64, String> {
        let (mut config, version) = self.service.get_agent_config(id)?;
        config
            .llm
            .get_or_insert(AgentLlmConfigFile {
                model: None,
                fallback_models: None,
                overrides: None,
            })
            .model = Some(model.to_string());
        self.service.update_agent_config(id, config, version)
    }

    /// The model `GET /v1/agents/{id}/config` reports.
    fn config_model(&self, id: &str) -> Option<String> {
        let (config, _) = self.service.get_agent_config(id).unwrap();
        config.llm.and_then(|l| l.model)
    }

    /// One run of the template: spawned, its definition read, ended.
    fn run(&self, id: &str, task: &str) -> SubAgent {
        let agent = self.registry.spawn_instance(id, task.to_string()).unwrap();
        self.registry.destroy_instance(&agent.id);
        agent
    }
}

fn lead(model: &str, description: &str) -> String {
    format!(
        "---\nid: lead_agent\nname: Lead\ndescription: {description}\nsingleton: true\n\
         model: {model}\nmax_rounds: 7\ncapabilities:\n  - \"orchestration\"\n---\n\n\
         ## Persona\n\nLeads.\n"
    )
}

fn reader(model: &str) -> String {
    format!(
        "---\nid: reader\nname: Reader\ndescription: reads\nmodel: {model}\nmax_rounds: 4\n\
         capabilities:\n  - \"file_read\"\n---\n\n## Persona\n\nReads.\n"
    )
}

/// The owner's words: a setting change "must not change the running
/// agent" but loads for its next run. A set on the singleton lead while it
/// runs leaves its instance busy with its task and its definition — the
/// singleton is not freed for a second run — and the next run has it.
#[test]
fn a_set_on_a_running_lead_leaves_its_run_and_reaches_the_next() {
    let store = Store::new(&[("lead_agent.md", lead("model-a", "leads"))]);
    let daemon = store.start();
    let running = daemon
        .registry
        .spawn_instance("lead_agent", "t1".to_string())
        .unwrap();

    daemon.set_model("lead_agent", "model-set").unwrap();

    let instance = daemon.registry.get_instance("lead_agent").unwrap();
    assert_eq!(
        instance.status,
        AgentStatus::Busy {
            task_id: "t1".to_string()
        }
    );
    assert_eq!(instance.current_task.as_deref(), Some("t1"));
    assert_eq!(instance.llm_config.model.as_deref(), Some("model-a"));
    assert!(
        daemon
            .registry
            .spawn_instance("lead_agent", "t2".to_string())
            .is_err(),
        "the running lead is not claimed a second time"
    );
    assert_eq!(
        daemon.config_model("lead_agent").as_deref(),
        Some("model-set")
    );

    daemon.registry.destroy_instance(&running.id);
    let next = daemon.run("lead_agent", "t3");
    assert_eq!(next.llm_config.model.as_deref(), Some("model-set"));
}

/// The twenty-eighth subsection's fault (i): a set on a template that is
/// not a singleton reached no run, every spawn building from the template.
#[test]
fn a_set_on_a_template_that_is_not_a_singleton_reaches_its_runs() {
    let store = Store::new(&[("reader.md", reader("model-r"))]);
    let daemon = store.start();

    daemon.set_model("reader", "model-set").unwrap();

    for task in ["t1", "t2"] {
        let run = daemon.run("reader", task);
        assert!(run.id.starts_with("reader::"), "{}", run.id);
        assert_eq!(run.llm_config.model.as_deref(), Some("model-set"));
    }
}

/// Item 22's second behaviour: the set carries the template's
/// `max_rounds`, which its round trip dropped.
#[test]
fn a_set_carries_the_templates_max_rounds() {
    let store = Store::new(&[
        ("lead_agent.md", lead("model-a", "leads")),
        ("reader.md", reader("model-r")),
    ]);
    let daemon = store.start();
    let (config, _) = daemon.service.get_agent_config("lead_agent").unwrap();
    assert_eq!(config.constraints.and_then(|c| c.max_rounds), Some(7));

    daemon.set_model("lead_agent", "model-set").unwrap();
    daemon.set_model("reader", "model-set").unwrap();

    assert_eq!(
        daemon.run("lead_agent", "t1").constraints.max_rounds,
        Some(7)
    );
    assert_eq!(daemon.run("reader", "t2").constraints.max_rounds, Some(4));
}

/// The lead's reading of item 22: the set is stored and loaded for the
/// runs after a restart, a bundled template's as a file's, and `GET
/// /v1/agents/{id}` (the row) and `GET …/config` agree on it before and
/// after.
#[test]
fn a_set_survives_a_restart_and_both_gets_agree_on_it() {
    let store = Store::new(&[("reader.md", reader("model-r"))]);
    let before = store.start();
    before.set_model("reader", "model-set").unwrap();
    before.set_model("general_agent", "model-general").unwrap();
    for (id, model) in [("reader", "model-set"), ("general_agent", "model-general")] {
        assert_eq!(store.row_model(id).as_deref(), Some(model));
        assert_eq!(before.config_model(id).as_deref(), Some(model));
    }
    drop(before);

    let after = store.start();

    for (id, model) in [("reader", "model-set"), ("general_agent", "model-general")] {
        assert_eq!(
            after.run(id, "t1").llm_config.model.as_deref(),
            Some(model),
            "{id}"
        );
        assert_eq!(store.row_model(id).as_deref(), Some(model), "{id}");
        assert_eq!(after.config_model(id).as_deref(), Some(model), "{id}");
    }
}

/// The lead's reading: the set outlasts a template edit — the fields it
/// set keep its values, every other field follows the template — and the
/// two GETs agree while the lead runs and after the edit.
#[test]
fn a_set_outlasts_a_template_edit_for_the_fields_it_set() {
    let store = Store::new(&[("lead_agent.md", lead("model-a", "leads"))]);
    let daemon = store.start();
    daemon.set_model("lead_agent", "model-set").unwrap();
    let running = daemon
        .registry
        .spawn_instance("lead_agent", "t1".to_string())
        .unwrap();
    assert_eq!(store.row_model("lead_agent").as_deref(), Some("model-set"));
    assert_eq!(
        daemon.config_model("lead_agent").as_deref(),
        Some("model-set")
    );

    store.rewrite(&daemon, "lead_agent.md", &lead("model-b", "leads, edited"));
    daemon.registry.destroy_instance(&running.id);

    let next = daemon.run("lead_agent", "t2");
    assert_eq!(next.llm_config.model.as_deref(), Some("model-set"));
    assert_eq!(next.description.as_deref(), Some("leads, edited"));
    assert_eq!(store.row_model("lead_agent").as_deref(), Some("model-set"));
    assert_eq!(
        daemon.config_model("lead_agent").as_deref(),
        Some("model-set")
    );
}

/// A set of a field back to the template's value drops it from the stored
/// set, so a later template edit of that field reaches the runs again.
#[test]
fn a_field_set_back_to_the_templates_value_follows_the_template_again() {
    let store = Store::new(&[("reader.md", reader("model-r"))]);
    let daemon = store.start();
    daemon.set_model("reader", "model-set").unwrap();
    assert!(
        SubAgentRepository::new(&store.db)
            .stored_set("reader")
            .unwrap()
            .is_some()
    );

    daemon.set_model("reader", "model-r").unwrap();

    assert_eq!(
        SubAgentRepository::new(&store.db)
            .stored_set("reader")
            .unwrap(),
        None
    );
    store.rewrite(&daemon, "reader.md", &reader("model-r2"));
    assert_eq!(
        daemon.run("reader", "t1").llm_config.model.as_deref(),
        Some("model-r2")
    );
}

/// The commit's choice, the no-change side (`tasks/pending-decisions.md`
/// §1): a set is the id's, so a template deleted and written again under
/// the same id runs with it.
#[test]
fn a_set_stays_with_its_id_when_the_template_is_deleted() {
    let store = Store::new(&[("reader.md", reader("model-r"))]);
    let daemon = store.start();
    daemon.set_model("reader", "model-set").unwrap();

    store.remove(&daemon, "reader.md");
    assert!(daemon.registry.get_template("reader").is_none());
    store.rewrite(&daemon, "reader.md", &reader("model-r2"));

    assert_eq!(
        daemon.run("reader", "t1").llm_config.model.as_deref(),
        Some("model-set")
    );
}

/// Beyond the owner's words (`tasks/pending-decisions.md` §1): a set
/// naming a run's own instance — `<template>::<id>`, which no later run
/// loads — would change only that running agent, so it is refused, naming
/// the template to set, and the instance stays busy.
#[test]
fn a_set_on_a_runs_own_instance_is_refused_and_leaves_it_running() {
    let store = Store::new(&[("reader.md", reader("model-r"))]);
    let daemon = store.start();
    let running = daemon
        .registry
        .spawn_instance("reader", "t1".to_string())
        .unwrap();

    let err = daemon.set_model(&running.id, "model-set").unwrap_err();

    assert!(err.starts_with("AGENT_RUNNING"), "{err}");
    assert!(err.contains("'reader'"), "{err}");
    let instance = daemon.registry.get_instance(&running.id).unwrap();
    assert!(!instance.status.is_available());
    assert_eq!(instance.llm_config.model.as_deref(), Some("model-r"));
    assert!(!store.agents().join(format!("{}.toml", running.id)).exists());
}

/// Item 22's third behaviour: `POST /v1/agents` with a template's id is
/// refused naming the template, not "already exists"; the reach of the
/// refusal is as before (an id with an instance).
#[test]
fn creating_an_agent_with_a_templates_id_is_refused_naming_the_template() {
    let store = Store::new(&[("lead_agent.md", lead("model-a", "leads"))]);
    let daemon = store.start();
    let (config, _) = daemon.service.get_agent_config("lead_agent").unwrap();

    let err = daemon.service.create_agent(config).unwrap_err();

    assert!(!err.contains("already exists"), "{err}");
    assert!(err.contains("template"), "{err}");
    assert!(err.contains("Lead"), "{err}");
    assert!(err.contains("lead_agent.md"), "{err}");
    assert!(!store.agents().join("lead_agent.toml").exists());
}

/// What `GET /v1/agents/{id}` shows of a definition: the row's.
fn row_definition(row: &openalpaca_storage::SubAgentConfig) -> serde_json::Value {
    let parse = |s: Option<&str>| -> serde_json::Value {
        s.map(|s| serde_json::from_str(s).unwrap())
            .unwrap_or(serde_json::Value::Null)
    };
    serde_json::json!({
        "name": row.name,
        "description": row.description,
        "icon": row.icon,
        "persona": row.persona,
        "skills": parse(Some(&row.skills_json)),
        "preset": parse(Some(&row.preset_json)),
        "constraints": parse(row.constraints_json.as_deref()),
        "llm": parse(row.llm_config_json.as_deref()),
    })
}

/// Item 22: `GET /v1/agents/{id}` (the row) and `GET …/config` agree on
/// the whole definition — constraints, `max_rounds` among them, the
/// capability lists and the rest — with no set stored and with one, a
/// file's template as a bundled one's.
#[test]
fn the_row_and_the_config_agree_with_no_set_and_with_one() {
    let store = Store::new(&[("reader.md", reader("model-r"))]);
    let daemon = store.start();
    let agree = |id: &str| {
        let (config, _) = daemon.service.get_agent_config(id).unwrap();
        let row = SubAgentRepository::new(&store.db).get(id).unwrap().unwrap();
        assert_eq!(
            row_definition(&row),
            row_definition(&config.into_storage_config()),
            "{id}"
        );
    };
    for id in ["reader", "research_agent"] {
        agree(id);
        daemon.set_model(id, "model-set").unwrap();
        agree(id);
    }
    let row = SubAgentRepository::new(&store.db)
        .get("reader")
        .unwrap()
        .unwrap();
    assert_eq!(row_definition(&row)["constraints"]["max_rounds"], 4);
}

/// Run SQL on the store's own connection: the faults below are planted
/// there, since a set's store write and read are the store's.
fn plant(store: &Store, sql: &str) {
    store
        .db
        .with_connection(|conn| Ok(conn.execute_batch(sql)?))
        .unwrap();
}

/// AGENT-SETS' refusal (`update_agent_config`): a set the store cannot
/// keep is refused and the registry put back as it was — the config read,
/// the next run and the row show the set it replaced, and a set after the
/// fault is gone is taken with the version read again.
#[test]
fn a_set_the_store_cannot_keep_is_refused_and_put_back() {
    let store = Store::new(&[("reader.md", reader("model-r"))]);
    let daemon = store.start();
    daemon.set_model("reader", "model-a").unwrap();
    plant(
        &store,
        "CREATE TRIGGER no_set BEFORE UPDATE OF config ON agent \
         BEGIN SELECT RAISE(ABORT, 'the store refuses the set'); END;",
    );

    let err = daemon.set_model("reader", "model-b").unwrap_err();

    assert!(err.contains("could not be saved"), "{err}");
    assert_eq!(daemon.config_model("reader").as_deref(), Some("model-a"));
    assert_eq!(
        daemon.run("reader", "t1").llm_config.model.as_deref(),
        Some("model-a")
    );
    assert_eq!(store.row_model("reader").as_deref(), Some("model-a"));
    assert_eq!(
        SubAgentRepository::new(&store.db)
            .stored_set("reader")
            .unwrap()
            .map(|set| set.contains("model-a")),
        Some(true)
    );

    plant(&store, "DROP TRIGGER no_set;");
    daemon.set_model("reader", "model-c").unwrap();
    assert_eq!(
        daemon.run("reader", "t2").llm_config.model.as_deref(),
        Some("model-c")
    );
}

/// The rescan (`file_layer::read_stored_set`): a store whose set cannot
/// be read leaves the set held in memory as it is, so the next run of a
/// template edited meanwhile still carries it.
#[test]
fn a_set_the_rescan_cannot_read_stays_held() {
    let store = Store::new(&[("reader.md", reader("model-r"))]);
    let daemon = store.start();
    daemon.set_model("reader", "model-set").unwrap();
    plant(&store, "ALTER TABLE agent RENAME COLUMN config TO unread;");
    assert!(
        SubAgentRepository::new(&store.db)
            .stored_set("reader")
            .is_err()
    );

    store.rewrite(&daemon, "reader.md", &reader("model-r2"));

    assert_eq!(
        daemon.run("reader", "t1").llm_config.model.as_deref(),
        Some("model-set")
    );
    assert_eq!(daemon.config_model("reader").as_deref(), Some("model-set"));
}

/// The rescan's other branch: a stored set that does not parse is logged
/// and the template runs as it is.
#[test]
fn a_stored_set_that_does_not_parse_leaves_the_template() {
    let store = Store::new(&[("reader.md", reader("model-r"))]);
    let daemon = store.start();
    daemon.set_model("reader", "model-set").unwrap();
    plant(
        &store,
        "UPDATE agent SET config = '{not json' WHERE id = 'reader';",
    );

    store.rewrite(&daemon, "reader.md", &reader("model-r2"));

    assert_eq!(
        daemon.run("reader", "t1").llm_config.model.as_deref(),
        Some("model-r2")
    );
}
