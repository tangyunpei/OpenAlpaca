use super::*;
use super::persona::{ensure_soul_file, ensure_soul_template_file};
use openalpaca_core::middleware::prompt::SystemPersona;
use tempfile::TempDir;

/// A fresh directory that is removed when the guard drops — on a failed
/// assert too. Hold the guard for the whole test and borrow `.path()`.
fn make_temp_dir(prefix: &str) -> TempDir {
    tempfile::Builder::new()
        .prefix(&format!("{prefix}-"))
        .tempdir()
        .expect("temp dir should be creatable")
}

#[test]
fn test_bootstrap_system_persona_creates_template_and_soul() {
    let tmp = make_temp_dir("openalpaca-soul-bootstrap");
    let dir = tmp.path();
    let (persona, soul_path) = bootstrap_system_persona(dir);

    assert_eq!(persona.name, "OpenAlpaca");
    assert!(soul_path.exists());
    assert!(
        dir.join("orchestrator")
            .join("templates")
            .join("SOUL_temp.md")
            .exists()
    );
}

#[test]
fn test_bootstrap_system_persona_falls_back_on_invalid_soul() {
    let tmp = make_temp_dir("openalpaca-soul-invalid");
    let dir = tmp.path();
    let template_path = ensure_soul_template_file(dir).expect("template should bootstrap");
    let soul_path = ensure_soul_file(dir, &template_path).expect("soul file should bootstrap");

    std::fs::write(&soul_path, "invalid").expect("test should write invalid soul");
    let (persona, loaded_path) = bootstrap_system_persona(dir);

    assert_eq!(loaded_path, soul_path);
    assert_eq!(persona.name, SystemPersona::default().name);
    assert_eq!(persona.core_values, SystemPersona::default().core_values);
}

#[test]
fn test_is_same_file_path_matches_identical_files() {
    let tmp = make_temp_dir("openalpaca-soul-path");
    let dir = tmp.path();
    let file = dir.join("SOUL.md");
    std::fs::write(&file, "x").expect("test file should be writable");

    let canonical = std::fs::canonicalize(&file).expect("file should canonicalize");
    assert!(is_same_file_path(&file, &canonical));
}

/// GAP-09 — a daemon killed mid-run leaves a task `running` and its lanes
/// open. The boot sequence sweeps the task first, which is what makes the
/// span sweep's condition true; running them in that order reports every
/// abandoned lane as `cancelled` / `"interrupted"`, and a second boot
/// (the idempotence the sweep promises) changes nothing.
#[test]
fn the_boot_sweeps_report_abandoned_lanes_as_interrupted() {
    use openalpaca_storage::{Database, NewSubagentSpan, SubagentSpanRepository};

    let tmp = make_temp_dir("openalpaca-span-sweep");
    let dir = tmp.path();
    let db = Database::open(&dir.join("test.db")).expect("open db");
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO task (id, title, status, priority, created_by, source_lane)
             VALUES ('t1', 'a run', 'running', 0, 'tester', 'user:cli')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    SubagentSpanRepository::new(&db)
        .open(NewSubagentSpan {
            id: "n1",
            task_id: "t1",
            template_id: "review_agent",
            agent_instance_id: "review_agent::a",
            objective: None,
        })
        .expect("open span");

    // The span sweep alone must not touch a lane whose task is still running.
    close_orphaned_spans(&db);
    let spans = SubagentSpanRepository::new(&db)
        .list_for_task("t1")
        .unwrap();
    assert_eq!(spans[0].state, "running");

    // The real boot order: tasks first, then their lanes.
    sweep_interrupted_runs(&db, None, "instance-under-test");
    // §5.6b — the run is `interrupted`, not `failed`, and the detail names the
    // incarnation that found it.
    let task = openalpaca_storage::repository::TaskRepository::new(&db)
        .get("t1")
        .unwrap()
        .unwrap();
    assert_eq!(task.status, openalpaca_storage::TaskStatus::Interrupted);
    assert!(
        task.result_summary
            .as_deref()
            .is_some_and(|s| s.contains("instance-under-test")),
        "{:?}",
        task.result_summary
    );
    close_orphaned_spans(&db);
    let spans = SubagentSpanRepository::new(&db)
        .list_for_task("t1")
        .unwrap();
    assert_eq!(spans[0].state, "cancelled");
    assert_eq!(spans[0].detail.as_deref(), Some("interrupted"));
    assert!(spans[0].ended_at.is_some());

    // Idempotent: the next boot leaves the row exactly as it is.
    let ended_at = spans[0].ended_at.clone();
    close_orphaned_spans(&db);
    let spans = SubagentSpanRepository::new(&db)
        .list_for_task("t1")
        .unwrap();
    assert_eq!(spans[0].ended_at, ended_at);

    drop(db);
}

// ── §5.6b: recovery, ordering, idempotence ──────────────────────────

use openalpaca_storage::repository::{FollowupRepository, TaskRepository};
use openalpaca_storage::{Database, TaskStatus};

/// One `running` run in `session`, with the shape the dispatcher writes.
fn seed_run(db: &Database, task_id: &str, session: &str, lane: &str) {
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO session (id, lane_key, source, status) VALUES (?1, ?2, 'gui', 'archived')",
            (session, lane),
        )?;
        conn.execute(
            "INSERT INTO task (id, title, status, priority, created_by, source_lane, session_id)
             VALUES (?1, 'a run', 'running', 0, 'tester', ?2, ?3)",
            (task_id, lane, session),
        )?;
        Ok(())
    })
    .unwrap();
}

/// The session log a `kill -9` leaves behind: two pushes, one of them drained
/// at the round boundary, and no `workflow_done`.
fn seed_crashed_log(root: &std::path::Path, session: &str, task_id: &str) {
    let dir = root.join(session);
    std::fs::create_dir_all(&dir).unwrap();
    let steering = |seq: u64, id: &str, text: &str| {
        serde_json::json!({
            "v": 1, "seq": seq, "ts": "2026-09-06T10:00:00.000Z",
            "type": "steering", "task_id": task_id,
            "data": {
                "request_id": id, "lane_key": "user:cli", "text": text,
                "received_at": "2026-09-06T10:00:00+00:00", "queue_depth": 1,
                "principal": {"User": {"global_id": "u-42"}},
                "workspace_path": "/repo",
            }
        })
        .to_string()
    };
    let drained = serde_json::json!({
        "v": 1, "seq": 3, "ts": "2026-09-06T10:00:01.000Z",
        "type": "steering_drained", "task_id": task_id,
        "data": {"at": "round_boundary", "round": 1, "count": 1, "request_ids": ["r-1"]}
    })
    .to_string();
    let body = format!(
        "{}\n{}\n{}\n",
        steering(1, "r-1", "delivered"),
        steering(2, "r-2", "never seen"),
        drained
    );
    std::fs::write(dir.join("log.jsonl"), body).unwrap();
}

fn queued_contents(db: &Database, lane: &str) -> Vec<String> {
    FollowupRepository::new(db)
        .list_queued_by_lane(lane)
        .unwrap()
        .into_iter()
        .map(|r| r.content)
        .collect()
}

/// §5.6b end to end: a run left `running` becomes `interrupted`, and the
/// interjection no `steering_drained` names is re-queued as an
/// `unprocessed_steering` follow-up. The one that *was* drained is not — the
/// model already saw it.
#[test]
fn the_boot_sweep_marks_the_run_interrupted_and_recovers_its_undrained_steering() {
    let tmp = make_temp_dir("openalpaca-interrupted");
    let dir = tmp.path();
    let db = Database::open(&dir.join("test.db")).unwrap();
    let sessions = dir.join("sessions");
    seed_run(&db, "t1", "s1", "user:cli");
    seed_crashed_log(&sessions, "s1", "t1");

    let runs = sweep_interrupted_runs(&db, Some(&sessions), "instance-7");

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].task_id, "t1");
    assert_eq!(runs[0].session_id.as_deref(), Some("s1"));
    assert_eq!(runs[0].recovered_steering, 1);

    let task = TaskRepository::new(&db).get("t1").unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Interrupted);
    assert!(task.status.is_terminal());
    assert!(
        task.result_summary
            .as_deref()
            .is_some_and(|s| s.contains("instance-7"))
    );

    assert_eq!(queued_contents(&db, "user:cli"), vec!["never seen"]);
    let row = &FollowupRepository::new(&db)
        .list_queued_by_lane("user:cli")
        .unwrap()[0];
    assert_eq!(row.kind, "unprocessed_steering");
    assert_eq!(row.source_task_id.as_deref(), Some("t1"));
    assert_eq!(row.session_id.as_deref(), Some("s1"));

    drop(db);
}

/// Twice, and only once. The second boot's `list_non_terminal` is empty
/// because `interrupted` is terminal — and even a *third* pass driven straight
/// at the same log (the crash window between the recovery and the flip) adds
/// nothing, because the row's own presence is the marker.
#[test]
fn a_second_boot_recovers_the_same_interjection_no_second_time() {
    let tmp = make_temp_dir("openalpaca-interrupted-twice");
    let dir = tmp.path();
    let db = Database::open(&dir.join("test.db")).unwrap();
    let sessions = dir.join("sessions");
    seed_run(&db, "t1", "s1", "user:cli");
    seed_crashed_log(&sessions, "s1", "t1");

    assert_eq!(
        sweep_interrupted_runs(&db, Some(&sessions), "boot-1")[0].recovered_steering,
        1
    );
    // Boot two: nothing is in flight any more, so nothing is even read.
    assert!(sweep_interrupted_runs(&db, Some(&sessions), "boot-2").is_empty());
    assert_eq!(queued_contents(&db, "user:cli"), vec!["never seen"]);

    // The crash window: the recovery ran, the flip did not. Put the row back
    // to `running` and sweep again — still one follow-up.
    db.with_connection(|conn| {
        conn.execute("UPDATE task SET status = 'running' WHERE id = 't1'", [])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        sweep_interrupted_runs(&db, Some(&sessions), "boot-3")[0].recovered_steering,
        0
    );
    assert_eq!(queued_contents(&db, "user:cli"), vec!["never seen"]);

    drop(db);
}

/// The order the brief fixes: **recovery before the byte-cap eviction**. The
/// interjection exists only in the JSONL, and R54 lets the sweep take an
/// archived session's live segment — so running the sweep first loses it.
///
/// Both halves are asserted on the same fixture, twice, so the assertion is
/// about the order and not about the fixture.
#[test]
fn the_recovery_runs_before_the_byte_cap_eviction_or_the_interjection_is_gone() {
    use openalpaca_core::session_log::sweep;

    let empty = std::collections::HashSet::new();

    // (a) The boot order as it ships: recover, then evict.
    let right = make_temp_dir("openalpaca-order-right");
    let dir = right.path();
    let db = Database::open(&dir.join("test.db")).unwrap();
    let sessions = dir.join("sessions");
    seed_run(&db, "t1", "s1", "user:cli");
    seed_crashed_log(&sessions, "s1", "t1");

    sweep_interrupted_runs(&db, Some(&sessions), "boot-1");
    // A cap of 0 bytes: the session is archived, so R54 lets its live segment
    // go, and the log is gone.
    sweep::enforce_total_cap(&sessions, 0, &empty, None).unwrap();
    assert!(!sessions.join("s1").join("log.jsonl").exists());
    assert_eq!(queued_contents(&db, "user:cli"), vec!["never seen"]);

    drop(db);
    drop(right);

    // (b) The same fixture with the two passes swapped: the eviction takes the
    // only copy of the interjection, and the recovery finds nothing.
    let wrong = make_temp_dir("openalpaca-order-wrong");
    let dir = wrong.path();
    let db = Database::open(&dir.join("test.db")).unwrap();
    let sessions = dir.join("sessions");
    seed_run(&db, "t1", "s1", "user:cli");
    seed_crashed_log(&sessions, "s1", "t1");

    sweep::enforce_total_cap(&sessions, 0, &empty, None).unwrap();
    assert!(
        !sessions.join("s1").join("log.jsonl").exists(),
        "the eviction really took the only copy"
    );
    let runs = sweep_interrupted_runs(&db, Some(&sessions), "boot-1");
    assert_eq!(runs[0].recovered_steering, 0);
    assert!(queued_contents(&db, "user:cli").is_empty());
    // The run is still honestly marked — the crash happened either way.
    assert_eq!(
        TaskRepository::new(&db).get("t1").unwrap().unwrap().status,
        TaskStatus::Interrupted
    );

    drop(db);
}

/// A run with no session (a lane that never had one, or a pre-039 row) is
/// still marked, has nothing to recover, and is not announced.
#[test]
fn a_run_with_no_session_is_marked_but_announces_nothing() {
    let tmp = make_temp_dir("openalpaca-interrupted-nosession");
    let dir = tmp.path();
    let db = Database::open(&dir.join("test.db")).unwrap();
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO task (id, title, status, priority, created_by, source_lane)
             VALUES ('t1', 'a run', 'queued', 0, 'tester', 'user:cli')",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let runs = sweep_interrupted_runs(&db, Some(&dir.join("sessions")), "boot-1");
    assert_eq!(runs.len(), 1);
    assert!(runs[0].session_id.is_none());

    let bus = openalpaca_core::bus::EventBus::default();
    let mut rx = bus.subscribe();
    announce_interrupted(&bus, &runs);
    assert!(rx.try_recv().is_err(), "no session, nothing to announce in");

    drop(db);
}

/// The frame §5.7 gained: `SessionChanged` with `status = "interrupted"` and
/// the run named, so a window open across the restart learns that the card it
/// was watching will never finish.
#[test]
fn an_interrupted_run_is_announced_on_its_session() {
    let tmp = make_temp_dir("openalpaca-interrupted-announce");
    let dir = tmp.path();
    let db = Database::open(&dir.join("test.db")).unwrap();
    seed_run(&db, "t1", "s1", "user:cli");
    let runs = sweep_interrupted_runs(&db, None, "boot-1");

    let bus = openalpaca_core::bus::EventBus::default();
    let mut rx = bus.subscribe();
    announce_interrupted(&bus, &runs);

    match rx.try_recv().expect("one frame") {
        openalpaca_core::events::SystemEvent::SessionChanged {
            session_id,
            lane_key,
            status,
            task_id,
            ..
        } => {
            assert_eq!(session_id, "s1");
            assert_eq!(lane_key, "user:cli");
            assert_eq!(status, "interrupted");
            assert_eq!(task_id.as_deref(), Some("t1"));
        }
        other => panic!("expected SessionChanged, got {other:?}"),
    }

    drop(db);
}

// ── L9: first-boot content ──────────────────────────────────────────────

/// A packaged daemon's config directory holds only what it seeds. Before L9
/// that was three files, and `GET /v1/agents` answered `[]` on a live install:
/// no template could act as Lead Agent, and no skill could be invoked.
///
/// Since the bundle (subagent design §2.5 S-1) the templates come from the
/// binary at every start, so a first boot writes none: `agents/` is created
/// empty, for the person's own and overriding files, while `tools/` is
/// seeded as before.
#[test]
fn a_first_boot_creates_an_empty_agents_directory_and_writes_no_template() {
    let tmp = make_temp_dir("openalpaca-seed-content");
    let dir = tmp.path();

    seed_default_configs(dir);

    assert!(dir.join("agents").is_dir(), "agents/ is created");
    assert_eq!(
        std::fs::read_dir(dir.join("agents")).unwrap().count(),
        0,
        "and no template is written into it"
    );

    assert!(dir.join("tools").join("example.toml").exists());
}

/// The skills are a bundled layer too (S-1, for `skills/`): run from the pack
/// the daemon writes under `state/cache/bundled-skills/`, so a first boot
/// writes none into the config directory — `skills/` is created empty, for
/// the person's own skills, their overrides and what `create-skill` writes.
#[test]
fn a_first_boot_creates_an_empty_skills_directory() {
    let tmp = make_temp_dir("openalpaca-seed-skills");
    let dir = tmp.path();

    seed_default_configs(dir);

    assert!(dir.join("skills").is_dir(), "skills/ is created");
    assert_eq!(
        std::fs::read_dir(dir.join("skills")).unwrap().count(),
        0,
        "and no skill is written into it"
    );
    assert!(dir.join("tools").join("example.toml").exists());
}

/// The daemon fills a gap; it does not restore a default. A directory the
/// owner has curated is theirs — including one they emptied.
#[test]
fn seeding_never_touches_content_the_owner_already_has() {
    let tmp = make_temp_dir("openalpaca-seed-content-existing");
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    std::fs::write(dir.join("agents").join("lead_agent.md"), "mine").unwrap();
    // Curated down to nothing: still the owner's directory.
    std::fs::create_dir_all(dir.join("skills")).unwrap();

    seed_default_configs(dir);

    assert_eq!(
        std::fs::read_to_string(dir.join("agents").join("lead_agent.md")).unwrap(),
        "mine",
        "an existing template is never overwritten"
    );
    assert_eq!(
        std::fs::read_dir(dir.join("agents")).unwrap().count(),
        1,
        "and the other eight are not added beside it"
    );
    assert_eq!(
        std::fs::read_dir(dir.join("skills")).unwrap().count(),
        0,
        "an emptied directory stays empty"
    );
    // …while the directory that really is absent is still filled.
    assert!(dir.join("tools").join("example.toml").exists());
}

/// Seeding is idempotent: a second boot writes nothing and changes nothing.
#[test]
fn a_second_boot_seeds_nothing_again() {
    let tmp = make_temp_dir("openalpaca-seed-content-twice");
    let dir = tmp.path();
    seed_default_configs(dir);
    std::fs::write(dir.join("agents").join("lead_agent.md"), "edited by hand").unwrap();

    seed_default_configs(dir);

    assert_eq!(
        std::fs::read_to_string(dir.join("agents").join("lead_agent.md")).unwrap(),
        "edited by hand"
    );
}

// ── N-SKILL-3: create-skill writes only where the daemon reads ───────────────

/// The daemon reads skills from one directory, `<config>/skills` (the
/// catalog's config layer in `services::initialize_services`). The built-in
/// `create-skill` runs from the pack under `state/cache/bundled-skills/`, so
/// the daemon tells its scripts where skills are written and where the
/// built-in ones are (`OPENALPACA_SKILLS_DIR`, `OPENALPACA_BUNDLED_SKILLS_DIR`,
/// subagent design §2.6 K-5); a copy in `config/skills` run by hand finds the
/// root from its own location. So each test runs both ways (K-7): from a pack
/// written into a scratch store with both variables set, as
/// `ScriptToolBuiltIn` sets them, and from a copy under a scratch
/// `config/skills` without them — the script itself, from its skill's
/// directory, with a scratch `HOME` and only `PATH` beside it. No script runs
/// in the repository.
#[cfg(unix)]
mod create_skill {
    use super::*;
    use openalpaca_core::bundled::{BUNDLED_SKILLS, write_skill_pack};
    use openalpaca_core::middleware::skill::SkillScope;
    use openalpaca_core::orchestrator::skill_catalog::SkillCatalog;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    /// Every directory and file under a root, each file with its bytes.
    type Tree = BTreeMap<PathBuf, Option<Vec<u8>>>;

    fn tree_of(root: &Path) -> Tree {
        fn walk(dir: &Path, root: &Path, out: &mut Tree) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                if std::fs::symlink_metadata(&path).unwrap().is_dir() {
                    out.insert(rel, None);
                    walk(&path, root, out);
                } else {
                    out.insert(rel, Some(std::fs::read(&path).unwrap()));
                }
            }
        }
        let mut out = Tree::new();
        walk(root, root, &mut out);
        out
    }

    /// Where the `create-skill` that runs comes from.
    #[derive(Clone, Copy, Debug)]
    enum Leg {
        /// The built-in one, from the pack, told both roots.
        Pack,
        /// A copy under `config/skills`, told nothing: its own location.
        ConfigCopy,
    }

    const LEGS: [Leg; 2] = [Leg::Pack, Leg::ConfigCopy];

    /// A first boot's config directory, `<tmp>/<config_name>`, the pack under
    /// `<tmp>/state/cache/bundled-skills`, and an empty `<tmp>/home`. The
    /// root is canonical, as the daemon's script path is
    /// (`ScriptToolBuiltIn::new`), so the paths a script prints compare equal.
    struct ScratchInstall {
        _guard: crate::test_util::HomeStoreGuard,
        _tmp: TempDir,
        root: PathBuf,
        config: PathBuf,
        pack: PathBuf,
        home: PathBuf,
        leg: Leg,
    }

    impl ScratchInstall {
        fn new(config_name: &str, leg: Leg) -> Self {
            let tmp = make_temp_dir("openalpaca-create-skill");
            let root = tmp.path().canonicalize().unwrap();
            let config = root.join(config_name);
            let home = root.join("home");
            std::fs::create_dir_all(&home).unwrap();
            let guard = crate::test_util::HomeStoreGuard::set(&root.join("store"));
            seed_default_configs(&config);
            let pack = root.join("state/cache/bundled-skills");
            write_skill_pack(&pack).unwrap();
            let install = Self {
                _guard: guard,
                _tmp: tmp,
                root,
                config,
                pack,
                home,
                leg,
            };
            if let Leg::ConfigCopy = leg {
                install.copy_create_skill_into_config();
            }
            install
        }

        /// This build's `create-skill`, as `<config>/skills/create-skill/`.
        fn copy_create_skill_into_config(&self) {
            for file in BUNDLED_SKILLS {
                let Some(rel) = file.path.strip_prefix("skills/create-skill/") else {
                    continue;
                };
                let target = self.skills_root().join("create-skill").join(rel);
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::write(&target, file.contents).unwrap();
                if rel.ends_with(".sh") {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
                        .unwrap();
                }
            }
        }

        /// The one directory the daemon reads skills from.
        fn skills_root(&self) -> PathBuf {
            self.config.join("skills")
        }

        /// The `create-skill` folder this leg runs.
        fn skill(&self) -> PathBuf {
            match self.leg {
                Leg::Pack => self.pack.join("create-skill"),
                Leg::ConfigCopy => self.skills_root().join("create-skill"),
            }
        }

        fn run(&self, script: &str, args: &[&str]) -> Output {
            self.run_from(&self.skill(), self.leg, script, args)
        }

        /// Run `script` of the `create-skill` at `skill`, told the roots as
        /// `leg` is.
        fn run_from(&self, skill: &Path, leg: Leg, script: &str, args: &[&str]) -> Output {
            let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
            let mut cmd = Command::new(skill.join("scripts").join(script));
            cmd.args(args)
                .current_dir(skill)
                .env_clear()
                .env("PATH", path)
                .env("HOME", &self.home);
            if let Leg::Pack = leg {
                cmd.env("OPENALPACA_SKILLS_DIR", self.skills_root())
                    .env("OPENALPACA_BUNDLED_SKILLS_DIR", &self.pack);
            }
            cmd.output().unwrap_or_else(|e| panic!("run {script}: {e}"))
        }
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// `--scope=user` wrote under `~/.config/openalpaca/skills`, a directory
    /// no build reads, outside the home store (D1), and reported success.
    /// Every script that takes a scope now refuses any scope but `project`,
    /// naming the root the daemon reads, prints no result, and writes
    /// nothing anywhere — under `HOME`, in the config directory or in the
    /// pack. The lister's old `both` is refused the same way. Each call gets
    /// a fresh install, so what one writes cannot hide what another does.
    #[test]
    fn every_scope_but_project_is_refused_naming_the_root_the_daemon_reads() {
        let calls: [(&str, &[&str]); 5] = [
            (
                "write_skill_md.sh",
                &[
                    "--id=hello-world",
                    "--name=Hello World",
                    "--description=Says hello",
                    "--scope=user",
                ],
            ),
            ("scaffold.sh", &["--id=hello-world", "--scope=user"]),
            ("check_exists.sh", &["--id=hello-world", "--scope=user"]),
            ("list_skills.sh", &["--scope=user"]),
            ("list_skills.sh", &["--scope=both"]),
        ];
        let mut failures = Vec::new();
        for leg in LEGS {
            for (script, args) in calls {
                let install = ScratchInstall::new("config", leg);
                let root = install.skills_root().display().to_string();
                let before = tree_of(&install.root);
                let out = install.run(script, args);
                let after = tree_of(&install.root);
                let mut wrong = Vec::new();
                if out.status.success() {
                    wrong.push("exit 0".to_string());
                }
                if !out.stdout.is_empty() {
                    wrong.push(format!("prints {:?}", text(&out.stdout)));
                }
                let stderr = text(&out.stderr);
                if !stderr.contains(&root) {
                    wrong.push(format!("does not name {root}: stderr {stderr:?}"));
                }
                let written: Vec<&PathBuf> = before
                    .keys()
                    .chain(after.keys())
                    .filter(|path| before.get(*path) != after.get(*path))
                    .collect();
                if !written.is_empty() {
                    wrong.push(format!("writes {written:?}"));
                }
                if !wrong.is_empty() {
                    failures.push(format!("{leg:?} {script} {args:?}: {}", wrong.join("; ")));
                }
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    /// With the default scope the new skill lands in `<config>/skills`, the
    /// checker finds it there, and the catalog the daemon builds from that
    /// directory loads it; nothing else is written — nothing in the pack.
    fn a_new_skill_lands_where_the_daemon_reads(config_name: &str, leg: Leg) {
        let install = ScratchInstall::new(config_name, leg);
        let root = install.skills_root();
        let skill_md = root.join("hello-world").join("SKILL.md");
        let before = tree_of(&install.root);

        let scaffold = install.run("scaffold.sh", &["--id=hello-world", "--dirs=scripts"]);
        assert!(
            scaffold.status.success(),
            "{leg:?}: {}",
            text(&scaffold.stderr)
        );
        let write = install.run(
            "write_skill_md.sh",
            &[
                "--id=hello-world",
                "--name=Hello World",
                "--description=Says hello",
                "--tools=file_read",
                "--scope=project",
            ],
        );
        assert!(write.status.success(), "{leg:?}: {}", text(&write.stderr));
        assert!(
            text(&write.stdout).contains(&skill_md.display().to_string()),
            "{leg:?}: the result names {}: {}",
            skill_md.display(),
            text(&write.stdout)
        );

        let after = tree_of(&install.root);
        let added: Vec<PathBuf> = after
            .keys()
            .filter(|path| !before.contains_key(*path))
            .cloned()
            .collect();
        let rel = |path: PathBuf| path.strip_prefix(&install.root).unwrap().to_path_buf();
        assert_eq!(
            added,
            vec![
                rel(root.join("hello-world")),
                rel(skill_md.clone()),
                rel(root.join("hello-world").join("scripts")),
            ],
            "{leg:?}: only the new skill is written"
        );
        for (path, contents) in &before {
            assert_eq!(
                after.get(path),
                Some(contents),
                "{leg:?}: {}",
                path.display()
            );
        }

        let exists = install.run("check_exists.sh", &["--id=hello-world"]);
        assert!(exists.status.success(), "{leg:?}: {}", text(&exists.stderr));
        assert_eq!(
            text(&exists.stdout).trim(),
            format!("{{\"exists\": true, \"path\": \"{}\"}}", skill_md.display()),
            "{leg:?}"
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(&root, SkillScope::Project);
        let entry = catalog
            .get("hello-world")
            .expect("the daemon's scan of <config>/skills loads the new skill");
        assert_eq!(entry.skill_md_path.as_deref(), Some(skill_md.as_path()));
    }

    #[test]
    fn a_new_skill_lands_in_the_config_directory_s_skills() {
        for leg in LEGS {
            a_new_skill_lands_where_the_daemon_reads("config", leg);
        }
    }

    /// `OPENALPACA_CONFIG_DIR` may name a directory of any name (the
    /// Installation Manual's *Runtime Overrides*); the scripts wrote to a
    /// `config/skills` beside it, which the daemon does not read.
    #[test]
    fn a_new_skill_lands_there_whatever_the_config_directory_is_called() {
        for leg in LEGS {
            a_new_skill_lands_where_the_daemon_reads("my-config", leg);
        }
    }

    /// The lister's output, parsed: `(id, slash, source)` per entry.
    fn listed(out: &Output) -> Vec<(String, String, String)> {
        assert!(out.status.success(), "{}", text(&out.stderr));
        let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", text(&out.stdout)));
        let mut rows: Vec<(String, String, String)> = rows
            .iter()
            .map(|row| {
                (
                    row["id"].as_str().unwrap().to_string(),
                    row["slash"].as_str().unwrap().to_string(),
                    row["source"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        rows.sort();
        rows
    }

    fn row(id: &str, slash: &str, source: &str) -> (String, String, String) {
        (id.to_string(), slash.to_string(), source.to_string())
    }

    /// K-6: create-skill's conflict check sees the built-in skills — run
    /// from the pack, the lister lists the config directory's skills and the
    /// pack's, each with its slash command and where it comes from.
    #[test]
    fn list_skills_lists_bundled_skills_and_their_slashes() {
        let install = ScratchInstall::new("config", Leg::Pack);
        let scaffold = install.run("scaffold.sh", &["--id=hello-world"]);
        assert!(scaffold.status.success(), "{}", text(&scaffold.stderr));
        std::fs::write(
            install.skills_root().join("hello-world/SKILL.md"),
            "---\nname: Hello\ndescription: d\ninvoke:\n  slash: /hi\n---\n",
        )
        .unwrap();

        assert_eq!(
            listed(&install.run("list_skills.sh", &[])),
            vec![
                row("code-review", "/review", "bundled"),
                row("commit-message", "/commit", "bundled"),
                row("create-skill", "/create-skill", "bundled"),
                row("explain-code", "/explain-code", "bundled"),
                row("hello-world", "/hi", "config"),
            ]
        );
    }

    /// K-6: an override of `create-skill` in `config/skills` — whose own
    /// skills directory is the config root — still lists the built-in skills
    /// when the daemon runs it, and its own folder as the config's.
    #[test]
    fn list_skills_from_an_override_create_skill_still_lists_the_bundled_ones() {
        let install = ScratchInstall::new("config", Leg::ConfigCopy);
        let out = install.run_from(
            &install.skills_root().join("create-skill"),
            Leg::Pack,
            "list_skills.sh",
            &[],
        );

        assert_eq!(
            listed(&out),
            vec![
                row("code-review", "/review", "bundled"),
                row("commit-message", "/commit", "bundled"),
                row("create-skill", "/create-skill", "bundled"),
                row("create-skill", "/create-skill", "config"),
                row("explain-code", "/explain-code", "bundled"),
            ]
        );
    }

    /// K-6: an id only a built-in skill holds is free to write, and is said
    /// to be a built-in one's — a new skill there overrides it, which the
    /// person should choose knowingly. The path is the config directory's.
    #[test]
    fn check_exists_says_when_only_a_bundled_skill_holds_the_id() {
        let install = ScratchInstall::new("config", Leg::Pack);
        let out = install.run("check_exists.sh", &["--id=code-review"]);
        assert!(out.status.success(), "{}", text(&out.stderr));
        assert_eq!(
            text(&out.stdout).trim(),
            format!(
                "{{\"exists\": false, \"bundled\": true, \"path\": \"{}\"}}",
                install.skills_root().join("code-review/SKILL.md").display()
            )
        );

        let out = install.run("check_exists.sh", &["--id=not-a-skill"]);
        assert_eq!(
            text(&out.stdout).trim(),
            format!(
                "{{\"exists\": false, \"path\": \"{}\"}}",
                install.skills_root().join("not-a-skill/SKILL.md").display()
            )
        );
    }
}
