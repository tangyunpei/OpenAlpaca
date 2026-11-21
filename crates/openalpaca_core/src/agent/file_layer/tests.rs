//! The file layer's rescan (subagent design §2.4 L-1a, L-3, L-4): every
//! change to `agents/` rereads the whole directory. Each test works in its
//! own temporary directory and database, never the repository's `config/`,
//! and holds the crate's home-store guard on a home inside it.

use super::*;
use crate::agent::registry::{DestroyOutcome, NotUsedReason, TemplateLayer, TemplateState};
use crate::test_util::HomeStoreGuard;
use openalpaca_storage::SubAgentRepository;
use tempfile::TempDir;

struct Fixture {
    // Declared first, so it drops first: the variable is restored before
    // the folders go.
    _guard: HomeStoreGuard,
    _dir: TempDir,
    agents: PathBuf,
    db: Database,
    registry: AgentRegistry,
    tools: ToolRegistry,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        let home = dir.path().canonicalize().unwrap().join("home");
        Self {
            _guard: HomeStoreGuard::set(&home),
            _dir: dir,
            agents,
            db,
            registry: AgentRegistry::new(),
            tools: ToolRegistry::default(),
        }
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.agents.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn rescan(&self, mode: LoadMode) -> Result<(), String> {
        rescan_file_layer(&self.agents, mode, &self.registry, &self.tools, &self.db)
    }

    fn ids(&self) -> Vec<String> {
        self.registry
            .list_templates()
            .into_iter()
            .map(|t| t.frontmatter.id)
            .collect()
    }
}

fn template(id: &str, model: &str) -> String {
    format!(
        "---\nid: {id}\nname: {id}\ndescription: the {id} template\nmodel: {model}\n\
         capabilities:\n  - \"file_read\"\n---\n\n## Persona\n\nTest.\n"
    )
}

fn lead(model: &str) -> String {
    format!(
        "---\nid: lead_agent\nname: Lead\ndescription: leads\nsingleton: true\nmodel: {model}\n\
         capabilities:\n  - \"orchestration\"\n---\n\n## Persona\n\nLeads.\n"
    )
}

/// L-3: a file deleted while the daemon runs leaves the catalog at the next
/// rescan — the watcher's event — where hot reload used to keep it until a
/// restart.
#[test]
fn a_template_file_deleted_while_the_daemon_runs_leaves_the_catalog() {
    let f = Fixture::new();
    let gone = f.write("reader.md", &template("reader", "m1"));
    f.write("writer.md", &template("writer", "m1"));
    f.rescan(LoadMode::Start).unwrap();
    assert_eq!(f.ids(), ["reader", "writer"]);

    std::fs::remove_file(gone).unwrap();
    f.rescan(LoadMode::Reload).unwrap();

    assert_eq!(f.ids(), ["writer"]);
    assert!(f.registry.provenance("reader").is_none());
    let err = f
        .registry
        .spawn_instance("reader", "t1".to_string())
        .unwrap_err();
    assert!(err.contains("not found"), "{err}");
}

/// L-3: a file given a new `id:` drops the old id; nothing keeps it.
#[test]
fn a_template_file_whose_id_changes_drops_the_old_id() {
    let f = Fixture::new();
    f.write("reader.md", &template("reader", "m1"));
    f.rescan(LoadMode::Start).unwrap();

    f.write("reader.md", &template("scanner", "m1"));
    f.rescan(LoadMode::Reload).unwrap();

    assert_eq!(f.ids(), ["scanner"]);
    assert_eq!(
        f.registry.provenance("scanner").and_then(|p| p.path),
        Some(f.agents.join("reader.md"))
    );
}

/// L-4: an edit that no longer parses withholds the id the file names — a
/// spawn of it is refused naming the file and the error — instead of keeping
/// the copy loaded before the edit.
#[test]
fn a_template_file_that_no_longer_parses_withholds_its_id() {
    let f = Fixture::new();
    let path = f.write("reader.md", &template("reader", "m1"));
    f.rescan(LoadMode::Start).unwrap();

    // The `name:` line goes: the file still says which id it is.
    f.write(
        "reader.md",
        "---\nid: \"reader\"\ndescription: half an edit\n---\n\nBody.\n",
    );
    f.rescan(LoadMode::Reload).unwrap();

    assert!(f.ids().is_empty(), "a withheld id is not offered");
    let err = f
        .registry
        .spawn_instance("reader", "t1".to_string())
        .unwrap_err();
    assert_eq!(
        err,
        format!(
            "template 'reader' is withheld: {} does not parse (Missing required frontmatter \
             field 'name'); fix it, or remove it",
            path.display()
        )
    );
    let provenance = f.registry.provenance("reader").expect("the file holds it");
    assert_eq!(provenance.layer, TemplateLayer::File);
    assert!(matches!(
        provenance.state,
        TemplateState::Withheld { path: ref p, .. } if *p == path
    ));
    assert_eq!(provenance.warnings.len(), 1, "{:?}", provenance.warnings);

    // A rescan of the same directory keeps it withheld.
    f.rescan(LoadMode::Reload).unwrap();
    assert!(
        f.registry
            .spawn_instance("reader", "t2".to_string())
            .is_err()
    );

    // Fixed, it is back.
    f.write("reader.md", &template("reader", "m2"));
    f.rescan(LoadMode::Reload).unwrap();
    assert_eq!(f.ids(), ["reader"]);
}

/// A broken file naming an id nothing holds is skipped with a `WARN`, as at
/// every start before (L-4: only a held id is withheld).
#[test]
fn a_broken_file_naming_an_id_nothing_holds_is_skipped() {
    let f = Fixture::new();
    f.write("draft.md", "---\nid: draft\n---\n");
    f.write("reader.md", &template("reader", "m1"));
    f.rescan(LoadMode::Start).unwrap();

    assert_eq!(f.ids(), ["reader"]);
    assert!(f.registry.provenance("draft").is_none());
}

/// L-3: on a hot reload an unknown `annotation:` capability withholds that
/// file's id with a `WARN`, where the start refuses to boot on it.
#[test]
#[tracing_test::traced_test]
fn an_unknown_annotation_capability_on_hot_reload_withholds_that_file() {
    let f = Fixture::new();
    f.write("reader.md", &template("reader", "m1"));
    f.rescan(LoadMode::Start).unwrap();

    let bad = "---\nid: reader\nname: reader\ndescription: d\ncapabilities:\n  - \
               \"annotation:bogus\"\n---\n";
    let path = f.write("reader.md", bad);
    f.rescan(LoadMode::Reload)
        .expect("a reload withholds, it does not fail");

    let err = f
        .registry
        .spawn_instance("reader", "t1".to_string())
        .unwrap_err();
    assert!(
        err.starts_with(&format!(
            "template 'reader' is withheld: {} names an unknown capability \
             (capabilities: unknown annotation capability 'annotation:bogus'",
            path.display()
        )),
        "{err}"
    );
    assert!(logs_contain(
        "template reader is withheld until it is fixed or removed"
    ));

    // The same file stops a start, as it always has.
    drop(f); // one guard at a time
    let fresh = Fixture::new();
    fresh.write("reader.md", bad);
    let err = fresh.rescan(LoadMode::Start).unwrap_err();
    assert!(
        err.starts_with(&format!(
            "{}: capabilities: unknown annotation capability 'annotation:bogus'",
            fresh.agents.join("reader.md").display()
        )),
        "{err}"
    );
}

/// L-3: of two files claiming one id the first by file name holds it, the
/// other is listed not used, and a `WARN` names both — at every rescan, not
/// in `read_dir` order.
#[test]
#[tracing_test::traced_test]
fn two_files_with_one_id_resolve_to_the_first_by_name_and_warn() {
    let f = Fixture::new();
    let second = f.write("b_reader.md", &template("reader", "second"));
    let first = f.write("a_reader.md", &template("reader", "first"));
    f.rescan(LoadMode::Start).unwrap();

    let reader = f.registry.get_template("reader").unwrap();
    assert_eq!(reader.frontmatter.model.as_deref(), Some("first"));
    let provenance = f.registry.provenance("reader").unwrap();
    assert_eq!(provenance.path, Some(first.clone()));
    assert_eq!(provenance.not_used.len(), 1);
    assert_eq!(provenance.not_used[0].path, Some(second.clone()));
    assert_eq!(provenance.not_used[0].reason, NotUsedReason::Duplicate);
    assert!(logs_contain(&format!(
        "Agent templates {} and {} both claim the id reader; {} is used",
        first.display(),
        second.display(),
        first.display()
    )));

    // The first one gone, the second holds the id.
    std::fs::remove_file(&first).unwrap();
    f.rescan(LoadMode::Reload).unwrap();
    let reader = f.registry.get_template("reader").unwrap();
    assert_eq!(reader.frontmatter.model.as_deref(), Some("second"));
}

/// L-1a: a hot edit of the singleton lead's file reaches its next spawn —
/// after one run, and again while it sits idle — because the rescan syncs
/// the idle instance the singleton is claimed from.
#[test]
fn a_hot_edit_reaches_the_singleton_leads_next_spawn() {
    let f = Fixture::new();
    f.write("lead_agent.md", &lead("model-a"));
    f.rescan(LoadMode::Start).unwrap();

    // The first run claims the instance the start registered; its end
    // sets it back to idle (LEAD-SINGLETON).
    let first = f
        .registry
        .spawn_instance("lead_agent", "t1".to_string())
        .unwrap();
    assert_eq!(first.llm_config.model.as_deref(), Some("model-a"));
    f.registry.destroy_instance(&first.id);

    // Edited after one run: the rescan rebuilds the idle instance.
    f.write("lead_agent.md", &lead("model-b"));
    f.rescan(LoadMode::Reload).unwrap();
    let second = f
        .registry
        .spawn_instance("lead_agent", "t2".to_string())
        .unwrap();
    assert_eq!(second.llm_config.model.as_deref(), Some("model-b"));
    f.registry.destroy_instance(&second.id);

    // A run with no rescan before it claims the same instance, which its
    // end sets back to idle.
    let third = f
        .registry
        .spawn_instance("lead_agent", "t3".to_string())
        .unwrap();
    assert_eq!(third.llm_config.model.as_deref(), Some("model-b"));
    f.registry.destroy_instance(&third.id);
    let idle = f.registry.get("lead_agent").expect("the singleton stays");
    assert!(idle.status.is_available(), "{:?}", idle.status);

    // Edited while it sits idle: the rescan rebuilds that instance.
    f.write("lead_agent.md", &lead("model-c"));
    f.rescan(LoadMode::Reload).unwrap();
    let fourth = f
        .registry
        .spawn_instance("lead_agent", "t4".to_string())
        .unwrap();
    assert_eq!(fourth.llm_config.model.as_deref(), Some("model-c"));
}

/// LEAD-SINGLETON (C2's item 2, taken 2026-11-12): the singleton lead's
/// idle instance is registered as a singleton, so a run sets it back to
/// idle instead of removing it — the instance `GET
/// /v1/agents/lead_agent/config` reads is there before the first run and
/// after each, at the same `config_version`, and an unchanged rescan keeps
/// it.
#[test]
fn the_leads_idle_instance_survives_a_run() {
    let f = Fixture::new();
    f.write("lead_agent.md", &lead("model-a"));
    f.rescan(LoadMode::Start).unwrap();
    let (_, version) = f
        .registry
        .get_with_version("lead_agent")
        .expect("there before the first run");

    for task in ["t1", "t2"] {
        let run = f
            .registry
            .spawn_instance("lead_agent", task.to_string())
            .unwrap();
        assert_eq!(run.id, "lead_agent");
        assert_eq!(
            f.registry.destroy_instance(&run.id),
            DestroyOutcome::ResetToIdle
        );
        let (after, kept) = f
            .registry
            .get_with_version("lead_agent")
            .expect("there after a run");
        assert!(after.status.is_available(), "{:?}", after.status);
        assert_eq!(after.current_task, None);
        assert_eq!(after.llm_config.model.as_deref(), Some("model-a"));
        assert_eq!(kept, version);
    }

    f.rescan(LoadMode::Reload).unwrap();
    let (_, kept) = f
        .registry
        .get_with_version("lead_agent")
        .expect("kept by a rescan");
    assert_eq!(kept, version);
}

/// LEAD-SINGLETON with L-1a: an edit made while the lead runs is in its idle
/// instance when the run ends — what `GET /v1/agents/lead_agent/config`
/// reads — and in its next run, not the definition the run started from.
#[test]
fn an_edit_while_the_lead_runs_reaches_its_idle_instance_and_next_run() {
    let f = Fixture::new();
    f.write("lead_agent.md", &lead("model-a"));
    f.rescan(LoadMode::Start).unwrap();
    let run = f
        .registry
        .spawn_instance("lead_agent", "t1".to_string())
        .unwrap();

    f.write("lead_agent.md", &lead("model-b"));
    f.rescan(LoadMode::Reload).unwrap();
    f.registry.destroy_instance(&run.id);

    let idle = f.registry.get("lead_agent").expect("there after the run");
    assert!(idle.status.is_available(), "{:?}", idle.status);
    assert_eq!(idle.llm_config.model.as_deref(), Some("model-b"));
    let next = f
        .registry
        .spawn_instance("lead_agent", "t2".to_string())
        .unwrap();
    assert_eq!(next.llm_config.model.as_deref(), Some("model-b"));
}

/// LEAD-SINGLETON with C3a (§2.4 L-4a): a lead switched off while it runs
/// has no idle instance once the run ends.
#[test]
fn a_lead_switched_off_while_it_runs_has_no_idle_instance_after() {
    let f = Fixture::new();
    f.write("lead_agent.md", &lead("model-a"));
    f.rescan(LoadMode::Start).unwrap();
    let run = f
        .registry
        .spawn_instance("lead_agent", "t1".to_string())
        .unwrap();

    f.write(
        "lead_agent.md",
        "---\nid: lead_agent\ndisabled: true\n---\n",
    );
    f.rescan(LoadMode::Reload).unwrap();
    assert!(
        f.registry.get(&run.id).is_some(),
        "a busy instance is left alone"
    );

    assert_eq!(
        f.registry.destroy_instance(&run.id),
        DestroyOutcome::Removed
    );
    assert!(f.registry.get_with_version("lead_agent").is_none());
    assert!(
        f.registry
            .spawn_instance("lead_agent", "t2".to_string())
            .is_err()
    );
}

/// L-1a: a hot edit rebuilds a template's idle instance — the one `GET
/// /v1/agents/{id}/config` reads — with no run in between, and moves its
/// `config_version`.
#[test]
fn a_hot_edit_rebuilds_the_idle_instance_the_config_route_reads() {
    let f = Fixture::new();
    f.write("reader.md", &template("reader", "model-a"));
    f.rescan(LoadMode::Start).unwrap();
    let (before, version) = f.registry.get_with_version("reader").unwrap();
    assert_eq!(before.llm_config.model.as_deref(), Some("model-a"));

    f.write("reader.md", &template("reader", "model-b"));
    f.rescan(LoadMode::Reload).unwrap();

    let (after, moved) = f.registry.get_with_version("reader").unwrap();
    assert_eq!(after.llm_config.model.as_deref(), Some("model-b"));
    assert!(after.status.is_available(), "{:?}", after.status);
    assert!(moved > version, "{moved} after {version}");
}

/// L-4 at the start an existing store meets: a file that does not parse
/// never takes its id from a file of the same rescan that parses, whichever
/// sorts first (a half-edited Finder copy `reader copy.md` sorts before
/// `reader.md`), as every start before loaded the one that parses. It is
/// listed not used, with a `WARN`; once the file that parses has gone, the
/// id the registry held is withheld.
#[test]
#[tracing_test::traced_test]
fn a_broken_file_never_takes_the_id_of_a_file_that_parses() {
    let f = Fixture::new();
    let copy = f.write(
        "reader copy.md",
        "---\nid: reader\ndescription: half\n---\n",
    );
    let reader = f.write("reader.md", &template("reader", "m1"));
    f.rescan(LoadMode::Start).unwrap();

    assert_eq!(f.ids(), ["reader"]);
    let provenance = f.registry.provenance("reader").unwrap();
    assert_eq!(provenance.path, Some(reader.clone()));
    assert_eq!(provenance.state, TemplateState::Ready);
    assert_eq!(provenance.not_used.len(), 1);
    assert_eq!(provenance.not_used[0].path, Some(copy.clone()));
    assert_eq!(provenance.not_used[0].reason, NotUsedReason::DoesNotParse);
    assert!(logs_contain(&format!(
        "Failed to parse agent template {}: Missing required frontmatter field 'name'; {} \
         holds the id reader",
        copy.display(),
        reader.display()
    )));

    // The same directory again: the same outcome.
    f.rescan(LoadMode::Reload).unwrap();
    assert_eq!(f.ids(), ["reader"]);

    // The file that parses gone: the held id is withheld, and stays so.
    std::fs::remove_file(&reader).unwrap();
    for _ in 0..2 {
        f.rescan(LoadMode::Reload).unwrap();
        assert!(f.ids().is_empty());
        let state = f.registry.provenance("reader").unwrap().state;
        assert!(
            matches!(state, TemplateState::Withheld { ref path, .. } if *path == copy),
            "{state:?}"
        );
    }
}

/// L-1a: the idle instance a template file materialised goes with the file;
/// while busy it stays, and its run's end (the singleton lead's,
/// LEAD-SINGLETON) or the next change takes it.
#[test]
fn a_template_files_idle_instance_leaves_with_it() {
    let f = Fixture::new();
    f.write("reader.md", &template("reader", "m1"));
    let lead_path = f.write("lead_agent.md", &lead("model-a"));
    f.rescan(LoadMode::Start).unwrap();
    assert!(f.registry.get_with_version("reader").is_some());
    assert!(f.registry.get_with_version("lead_agent").is_some());

    let busy = f
        .registry
        .spawn_instance("lead_agent", "t1".to_string())
        .unwrap();
    std::fs::remove_file(f.agents.join("reader.md")).unwrap();
    std::fs::remove_file(lead_path).unwrap();
    f.rescan(LoadMode::Reload).unwrap();

    assert!(f.registry.get_with_version("reader").is_none());
    assert!(
        f.registry.get(&busy.id).is_some(),
        "a busy instance is left alone"
    );

    f.registry.destroy_instance(&busy.id);
    f.rescan(LoadMode::Reload).unwrap();
    assert!(
        f.registry.get(&busy.id).is_none(),
        "the next change takes it"
    );
}

/// L-3: the rescan writes the `agent` row of a template whose content
/// changed — the row `GET /v1/agents` lists.
#[test]
fn a_hot_edit_updates_the_agent_row() {
    let f = Fixture::new();
    f.write("reader.md", &template("reader", "model-a"));
    f.rescan(LoadMode::Start).unwrap();

    f.write("reader.md", &template("reader", "model-b"));
    f.rescan(LoadMode::Reload).unwrap();

    let row = SubAgentRepository::new(&f.db)
        .list(50)
        .unwrap()
        .into_iter()
        .find(|c| c.id == "reader")
        .expect("the row is written");
    let llm: serde_json::Value =
        serde_json::from_str(row.llm_config_json.as_deref().unwrap()).unwrap();
    assert_eq!(llm["model"], "model-b");
}

/// L-3: the rescan is idempotent — the watcher's event after a route's write
/// rescans again and changes nothing: no instance is rebuilt, so no
/// `config_version` moves under a client that read one.
#[test]
fn a_rescan_of_an_unchanged_directory_changes_nothing() {
    let f = Fixture::new();
    f.write("reader.md", &template("reader", "m1"));
    f.write("broken.md", "---\nid: reader2\n---\n");
    f.rescan(LoadMode::Start).unwrap();
    let (_, version) = f.registry.get_with_version("reader").unwrap();

    f.rescan(LoadMode::Reload).unwrap();
    f.rescan(LoadMode::Reload).unwrap();

    assert_eq!(f.registry.get_with_version("reader").unwrap().1, version);
    assert_eq!(f.ids(), ["reader"]);
    assert!(f.registry.provenance("reader2").is_none());
}
