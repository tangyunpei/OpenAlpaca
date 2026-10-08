//! The delete core against a scenario with a bystander on every leg: a second
//! project, a home conversation, a surviving conversation that also holds a
//! message naming the doomed project, and files linked from inside and from
//! outside. The home store is a tempdir behind the crate's one guard; both
//! projects are git repositories made for the test.

use super::*;
use crate::memory_files::{
    ControlSnapshot, NOTES_DIR, OWNERS_DIR, SnapshotScope, home_partition_key, owner_key,
    project_partition_key, snapshot_path_for,
};
use crate::repository::{ARTIFACT_ROLE, ATTACHMENT_ROLE};
use crate::store::StoreScope;
use crate::store::projects::{self, Project};
use crate::store::tests::HomeStoreGuard;
use crate::{
    ArtifactKind, ArtifactStore, Database, FileAssetRepository, NewArtifact, NewUpload,
    PurgeCounts, PurgeSession, UploadStore,
};
use chrono::{DateTime, TimeZone, Utc};
use std::path::PathBuf;
use tempfile::{TempDir, tempdir};

const OWNER: &str = "owner-1";

/// One date for every write, so a second put of a title is its next version.
fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0).unwrap()
}

pub(super) struct World {
    tmp: TempDir,
    _env: HomeStoreGuard,
    pub(super) home: PathBuf,
    pub(super) db: Database,
}

impl World {
    pub(super) fn new() -> Self {
        let tmp = tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let env = HomeStoreGuard::set(&home);
        let db = Database::open(&tmp.path().join("test.db")).unwrap();
        Self {
            tmp,
            _env: env,
            home,
            db,
        }
    }

    /// A git repository under the test's tempdir, registered as a project.
    pub(super) fn project(&self, name: &str) -> Project {
        let root = self
            .tmp
            .path()
            .canonicalize()
            .unwrap()
            .join("repos")
            .join(name);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        projects::register(&root).unwrap()
    }

    pub(super) fn exec(&self, sql: &str, params: &[&dyn rusqlite::ToSql]) {
        self.db
            .with_connection(|conn| {
                conn.execute(sql, params)?;
                Ok(())
            })
            .unwrap();
    }

    pub(super) fn count(&self, sql: &str) -> i64 {
        self.db
            .with_connection(|conn| Ok(conn.query_row(sql, [], |row| row.get::<_, i64>(0))?))
            .unwrap()
    }

    /// A user row in `session`, naming `root` in `workspace_root` (or none).
    pub(super) fn message(&self, session: &str, root: Option<&str>) -> i64 {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO conversation_messages
                         (lane_key, role, content, session_id, workspace_root)
                     VALUES (?1, 'user', 'hi', ?1, ?2)",
                    rusqlite::params![session, root],
                )?;
                Ok(conn.last_insert_rowid())
            })
            .unwrap()
    }

    /// An upload in `project`'s store whose bytes are its own name.
    pub(super) fn upload(&self, project: &Project, name: &str) -> String {
        let scope = StoreScope::Project(project.root.clone());
        UploadStore::new(&self.db)
            .put(NewUpload {
                owner_id: OWNER,
                filename: name,
                mime_type: "text/plain",
                data: name.as_bytes(),
                scope: &scope,
                created: at(),
            })
            .unwrap()
            .asset
            .id
    }

    /// A by-product in `project`'s store; the same title again is its next version.
    pub(super) fn by_product(&self, project: &Project, title: &str, bytes: &[u8]) -> String {
        let scope = StoreScope::Project(project.root.clone());
        let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, title, bytes);
        new.created = at();
        ArtifactStore::new(&self.db).put(new).unwrap().0.id
    }

    pub(super) fn link(&self, message: i64, file: &str, role: &str) {
        FileAssetRepository::new(&self.db)
            .link_to_message_with_role(message, file, 0, None, role)
            .unwrap();
    }

    /// A control snapshot written by hand: `project` names a project
    /// partition stamped with its id, `None` the home partition.
    pub(super) fn snapshot(&self, owner: &str, project: Option<&Project>) {
        let okey = owner_key(owner);
        let now = Utc::now();
        let (key, scope, dir, id) = match project {
            Some(p) => (
                project_partition_key(&okey, &p.id),
                SnapshotScope::Project,
                p.dir.join("memory").join(OWNERS_DIR).join(&okey),
                Some(p.id),
            ),
            None => (
                home_partition_key(&okey),
                SnapshotScope::Home,
                self.home.join("memory").join(OWNERS_DIR).join(&okey),
                None,
            ),
        };
        let mut snapshot =
            ControlSnapshot::new(&key, scope, &okey, owner, dir.to_string_lossy(), now);
        if let Some(id) = id {
            snapshot = snapshot.with_project_id(id);
        }
        snapshot
            .save(&snapshot_path_for(&key).unwrap(), now)
            .unwrap();
    }

    /// A file under a project partition's `notes/` — counted by name only.
    pub(super) fn note_file(&self, project: &Project, owner: &str, name: &str) {
        let dir = project
            .dir
            .join("memory")
            .join(OWNERS_DIR)
            .join(owner_key(owner))
            .join(NOTES_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), b"---\nschema: 1\n---\n").unwrap();
    }
}

/// What [`seed`] made, by name.
pub(super) struct Seed {
    pub(super) p: Project,
    pub(super) q: Project,
    pub(super) m_p: i64,
    pub(super) m_q_leg: i64,
    pub(super) m_q: i64,
    pub(super) m_h: i64,
    pub(super) u_alone: String,
    pub(super) u_shared: String,
    pub(super) u_leg: String,
    pub(super) u_q: String,
    pub(super) a_alone: String,
    pub(super) a_shared: String,
}

/// `p` is deleted; `q` and the home scope are bystanders.
///
/// | Row | Names `p` by | Fate |
/// |---|---|---|
/// | session `s-p` | `workspace_id` | goes |
/// | message `m_p` | its session | goes |
/// | message `m_q_leg` (session `s-q`) | `workspace_root` | goes; `s-q` stays |
/// | `t-p`, its span, event, decision, LLM call | `workspace_id` / the run | go |
/// | follow-ups `f-sp` (session `s-p`), `f-path` (session `s-q`, `workspace_path = p`) | session / path | go |
/// | `learning_cursor` of `s-p` | its session | goes |
/// | `u_alone` (linked by `m_p`), `u_leg` (linked by `m_q_leg`), `a_alone` (unlinked) | `project_root` | go |
/// | `u_shared` (linked by `m_p` and the home message `m_h`) | `project_root` | kept |
/// | `a_shared`, two versions (linked as an artifact by `m_q`) | `project_root` | kept, newest version |
pub(super) fn seed(w: &World) -> Seed {
    let p = w.project("doomed");
    let q = w.project("bystander");
    let (pr, qr) = (p.root_string(), q.root_string());
    w.exec(
        "INSERT INTO session (id, lane_key, source, workspace_id) VALUES ('s-p', 's-p', 'gui', ?1)",
        &[&pr],
    );
    w.exec(
        "INSERT INTO session (id, lane_key, source, workspace_id, message_count)
         VALUES ('s-q', 's-q', 'gui', ?1, 2)",
        &[&qr],
    );
    w.exec(
        "INSERT INTO session (id, lane_key, source) VALUES ('s-h', 's-h', 'gui')",
        &[],
    );
    let m_p = w.message("s-p", Some(pr.as_str()));
    let m_q_leg = w.message("s-q", Some(pr.as_str()));
    let m_q = w.message("s-q", Some(qr.as_str()));
    let m_h = w.message("s-h", None);
    w.exec(
        "INSERT INTO message_feedback (message_id, feedback) VALUES (?1, 'positive')",
        &[&m_p],
    );
    w.exec(
        "INSERT INTO task (id, title, created_by, source_lane, workspace_id, session_id, status)
         VALUES ('t-p', 't', 'test', 'test', ?1, 's-p', 'completed')",
        &[&pr],
    );
    w.exec(
        "INSERT INTO task (id, title, created_by, source_lane, workspace_id, session_id, status)
         VALUES ('t-q', 't', 'test', 'test', ?1, 's-q', 'completed')",
        &[&qr],
    );
    // A home turn that started p's run: the run goes, the turn stays readable.
    w.exec(
        "UPDATE conversation_messages SET task_id = 't-p' WHERE id = ?1",
        &[&m_h],
    );
    for (session, task) in [
        (Some("s-p"), None),
        (None, Some("t-p")),
        (Some("s-q"), None),
    ] {
        w.exec(
            "INSERT INTO tool_execution_log (agent_id, tool_name, success, duration_ms, session_id, task_id)
             VALUES ('a', 'read', 1, 3, ?1, ?2)",
            &[&session, &task],
        );
    }
    w.exec(
        "INSERT INTO lane_followups (lane_key, kind, content, principal_json, session_id)
         VALUES ('s-p', 'followup', 'later', '{}', 's-p')",
        &[],
    );
    w.exec(
        "INSERT INTO lane_followups (lane_key, kind, content, principal_json, session_id, workspace_path)
         VALUES ('s-q', 'followup', 'later', '{}', 's-q', ?1)",
        &[&pr],
    );
    w.exec(
        "INSERT INTO lane_followups (lane_key, kind, content, principal_json, session_id, workspace_path)
         VALUES ('s-q', 'followup', 'later', '{}', 's-q', ?1)",
        &[&qr],
    );
    w.exec(
        "INSERT INTO learning_cursor (session_id) VALUES ('s-p'), ('s-q')",
        &[],
    );
    w.exec(
        "INSERT INTO subagent_span (id, task_id, template_id, agent_instance_id, label, started_at)
         VALUES ('span-1', 't-p', 't', 'i', 'review', '2026-10-01T09:00:00Z')",
        &[],
    );
    w.exec(
        "INSERT INTO event_log (event_type, task_id) VALUES ('started', 't-p'), ('started', 't-q')",
        &[],
    );
    w.exec(
        "INSERT INTO dispatch_decisions (request_id, task_id, mode, reason)
         VALUES ('r-1', 't-p', 'lead', 'because')",
        &[],
    );
    w.exec(
        "INSERT INTO llm_call_log (task_id, provider, model) VALUES ('t-p', 'anthropic', 'sonnet')",
        &[],
    );

    let u_alone = w.upload(&p, "alone.txt");
    let u_shared = w.upload(&p, "shared.txt");
    let u_leg = w.upload(&p, "leg.txt");
    let u_q = w.upload(&q, "theirs.txt");
    let a_alone = w.by_product(&p, "Alone", b"alone\n");
    let a_shared = w.by_product(&p, "Shared", b"v1\n");
    assert_eq!(
        w.by_product(&p, "Shared", b"v2\n"),
        a_shared,
        "the second put is version 2"
    );
    w.link(m_p, &u_alone, ATTACHMENT_ROLE);
    w.link(m_p, &u_shared, ATTACHMENT_ROLE);
    w.link(m_h, &u_shared, ATTACHMENT_ROLE);
    w.link(m_q_leg, &u_leg, ATTACHMENT_ROLE);
    w.link(m_q, &a_shared, ARTIFACT_ROLE);
    w.link(m_q, &u_q, ATTACHMENT_ROLE);

    Seed {
        p,
        q,
        m_p,
        m_q_leg,
        m_q,
        m_h,
        u_alone,
        u_shared,
        u_leg,
        u_q,
        a_alone,
        a_shared,
    }
}

#[test]
fn the_plan_counts_every_row_the_five_columns_name_and_the_files_a_surviving_message_links() {
    let w = World::new();
    let s = seed(&w);

    let plan = delete_plan(&w.db, &s.p).unwrap();

    assert_eq!(
        plan.counts,
        PurgeCounts {
            sessions: 1,
            messages: 2,
            tool_calls: 2,
            followups: 2,
            tasks: 1,
            spans: 1,
            run_events: 1,
            uploads: 2,
            artifacts: 1,
        }
    );
    assert_eq!(
        plan.kept,
        KeptFiles {
            uploads: 1,
            artifacts: 1
        }
    );
    assert_eq!(
        plan.sessions,
        vec![PurgeSession {
            id: "s-p".to_string(),
            lane_key: "s-p".to_string()
        }]
    );
    assert_eq!(plan.project, s.p);
    assert_ne!(s.q.id, s.p.id);
    // Every row the scenario named is still there to be accounted for.
    for id in [
        &s.u_alone,
        &s.u_shared,
        &s.u_leg,
        &s.u_q,
        &s.a_alone,
        &s.a_shared,
    ] {
        assert_eq!(
            w.count(&format!(
                "SELECT COUNT(*) FROM file_assets WHERE id = '{id}'"
            )),
            1
        );
    }
    for id in [s.m_p, s.m_q_leg, s.m_q, s.m_h] {
        assert_eq!(
            w.count(&format!(
                "SELECT COUNT(*) FROM conversation_messages WHERE id = {id}"
            )),
            1
        );
    }
    // A plan reads only.
    assert_eq!(w.count("SELECT COUNT(*) FROM session"), 3);
    assert_eq!(w.count("SELECT COUNT(*) FROM conversation_messages"), 4);
    assert_eq!(
        w.count("SELECT COUNT(*) FROM file_assets"),
        6,
        "a second version is no second row"
    );
}

/// The kept set reads the same predicate the delete does: a message dies on
/// its session **or** on its own `workspace_root`, so a file only such a
/// message links is not kept — until a message the delete leaves links it.
#[test]
fn a_file_linked_only_by_a_message_naming_the_project_is_not_kept() {
    let w = World::new();
    let s = seed(&w);
    let before = delete_plan(&w.db, &s.p).unwrap();

    w.link(s.m_h, &s.u_leg, ATTACHMENT_ROLE);
    let after = delete_plan(&w.db, &s.p).unwrap();

    assert_eq!(after.kept.uploads, before.kept.uploads + 1);
    assert_eq!(after.counts.uploads, before.counts.uploads - 1);
}

#[test]
fn the_plan_counts_every_owners_partition_and_note_files_of_this_project_only() {
    let w = World::new();
    let p = w.project("noted");
    let q = w.project("other");
    w.snapshot("alice", Some(&p));
    w.snapshot("bob", Some(&p));
    w.snapshot("alice", Some(&q));
    w.snapshot("alice", None);
    w.note_file(&p, "alice", "00000000-0000-4000-8000-000000000001--a.md");
    w.note_file(&p, "alice", "00000000-0000-4000-8000-000000000002--b.md");
    w.note_file(&p, "bob", "00000000-0000-4000-8000-000000000003--c.md");
    w.note_file(&p, "bob", ".00000000-0000-4000-8000-000000000003--c.md.tmp");
    w.note_file(&q, "alice", "00000000-0000-4000-8000-000000000004--d.md");

    let plan = delete_plan(&w.db, &p).unwrap();

    assert_eq!(plan.memory_partitions, 2);
    assert_eq!(plan.memory_notes, 3);
    assert!(plan.counts.is_empty());
    assert_eq!(plan.kept, KeptFiles::default());
}
