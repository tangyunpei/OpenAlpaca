//! `MemoryService` tests — the slice-3 rows of the invariant fixtures
//! (`crates/openalpaca_storage/tests/fixtures/memory/`, cited per test) and
//! the service's own contracts. Every partition lives under a temp root and
//! the home store is `OPENALPACA_HOME_STORE` for the test's duration; the
//! index is in memory; no model, no network.

use super::*;
use crate::security::policy::Principal;
use crate::test_util::HomeStoreGuard;
use crate::tools::registry::ToolContext;
use openalpaca_storage::Database;
use openalpaca_storage::IdentityRepository;
use openalpaca_storage::memory_files::{
    ControlSnapshot, NoteKind, NoteScope, NoteSource, Origin, Partition, SourceType,
};
use openalpaca_storage::memory_index::MemoryIndex;
use openalpaca_storage::store::{StoreScope, ensure_store};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) const ALICE: &str = "u_example_alice";
pub(super) const BOB: &str = "u_example_bob";
pub(super) const ALICE_KEY: &str = "32acea7eb739b08afd959db91e46d450";

pub(super) struct Harness {
    _env: HomeStoreGuard,
    pub root: tempfile::TempDir,
    pub db: Database,
    pub service: MemoryService,
}

impl Harness {
    pub(super) fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let env = HomeStoreGuard::set(&home.canonicalize().unwrap());
        let db = Database::open(&root.path().join("db.sqlite")).unwrap();
        let identities = IdentityRepository::new(&db);
        identities.create_global_user(ALICE, None).unwrap();
        identities.create_global_user(BOB, None).unwrap();
        let service = MemoryService::new(Some(db.clone()), MemoryIndex::open_in_memory().unwrap());
        Self {
            _env: env,
            root,
            db,
            service,
        }
    }

    pub(super) fn home(&self) -> PathBuf {
        self.root.path().join("home").canonicalize().unwrap()
    }

    /// A workspace directory under the temp root with an optional marker.
    pub(super) fn workspace(&self, name: &str, marker: Option<&str>) -> PathBuf {
        let dir = self.root.path().join("ws").join(name);
        fs::create_dir_all(&dir).unwrap();
        if let Some(m) = marker {
            fs::create_dir_all(dir.join(m)).unwrap();
        }
        dir.canonicalize().unwrap()
    }

    pub(super) fn ctx(
        &self,
        owner: &str,
        workspace: Option<&Path>,
        surface: Surface,
        caps: Capabilities,
    ) -> Result<MemoryAccessContext, MemoryError> {
        let hint = match workspace {
            Some(p) => WorkspaceHint::Path(p),
            None => WorkspaceHint::None,
        };
        self.service
            .context(owner, hint, surface, caps, Attribution::default())
    }

    pub(super) fn tool(&self, owner: &str, workspace: Option<&Path>) -> MemoryAccessContext {
        self.ctx(owner, workspace, Surface::ModelTool, Capabilities::ALL)
            .unwrap()
    }

    pub(super) fn user(&self, owner: &str, workspace: Option<&Path>) -> MemoryAccessContext {
        self.ctx(owner, workspace, Surface::UserSurface, Capabilities::ALL)
            .unwrap()
    }

    pub(super) fn store(
        &self,
        ctx: &MemoryAccessContext,
        title: &str,
        body: &str,
        kind: NoteKind,
    ) -> StoreOutput {
        self.service
            .store(
                ctx,
                StoreInput {
                    title: title.into(),
                    body: body.into(),
                    kind: Some(kind),
                    ..Default::default()
                },
            )
            .unwrap()
    }

    pub(super) fn list_ids(&self, ctx: &MemoryAccessContext, input: ListInput) -> HashSet<NoteId> {
        self.service
            .list(ctx, input)
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.id)
            .collect()
    }

    pub(super) fn snapshot(&self, scope: &StoreScope, owner: &str) -> Option<ControlSnapshot> {
        Partition::open(scope, owner).unwrap().snapshot()
    }

    pub(super) fn home_notes_dir(&self, owner_key: &str) -> PathBuf {
        self.home()
            .join("memory")
            .join("owners")
            .join(owner_key)
            .join("notes")
    }

    pub(super) fn message_row(&self, text: &str) -> i64 {
        self.db
            .with_connection(|c| {
                c.execute(
                    "INSERT INTO conversation_messages (lane_key, role, content) VALUES ('u:cli', 'user', ?1)",
                    [text],
                )?;
                Ok(c.last_insert_rowid())
            })
            .unwrap()
    }
}

fn note_file_for(dir: &Path, id: NoteId) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        name.starts_with(&id.to_string()).then(|| e.path())
    })
}

// ============================================================================
// Owner (design §3; fixture 08)
// ============================================================================

#[test]
fn an_owner_that_is_not_a_global_user_row_has_no_partition() {
    let h = Harness::new();
    let ws = h.workspace("project_a", Some(".openalpaca"));
    for id in ["system", "provider:telegram:12345", "", "u_nobody"] {
        let err = h
            .ctx(id, Some(&ws), Surface::ModelTool, Capabilities::ALL)
            .expect_err("not a global_user row");
        assert_eq!(err.class(), "fixture:no_owner", "{id}: {err}");
    }
    // Nothing was created for them, not even an empty directory.
    for key in [
        "bbc5e661e106c6dcd8dc6dd186454c2f",
        "c00f1f5b28abd3f37a411afa16975510",
    ] {
        assert!(!h.home().join("memory").join("owners").join(key).exists());
        assert!(
            !h.home()
                .join("state")
                .join("memory-control")
                .join(format!("home-{key}.json"))
                .exists()
        );
    }
    assert!(!ws.join(".openalpaca").join("memory").exists());
    // A linked owner resolves.
    let ctx = h.tool(ALICE, None);
    assert_eq!(ctx.owner_key, ALICE_KEY);
    assert!(ctx.project_root.is_none());
}

#[test]
fn a_tool_context_takes_its_owner_from_the_principal_first() {
    let h = Harness::new();
    // A `/v1/command process` turn and every workflow it starts carry the
    // `System` principal: no owner, whatever `owner_id` says.
    let system = ToolContext {
        principal: Some(Principal::System),
        owner_id: Some(ALICE.to_string()),
        ..Default::default()
    };
    let err = h
        .service
        .context_for_tool(&system, Capabilities::ALL)
        .unwrap_err();
    assert_eq!(err.class(), "fixture:no_owner");
    // An unlinked external identity on a connector lane: no owner.
    let external = ToolContext {
        principal: Some(Principal::External {
            provider: "telegram".into(),
            id: "12345".into(),
        }),
        lane_key: Some("u_example_alice:telegram".into()),
        ..Default::default()
    };
    assert_eq!(
        h.service
            .context_for_tool(&external, Capabilities::ALL)
            .unwrap_err()
            .class(),
        "fixture:no_owner"
    );
    // The linked owner on the same lane: the home partition, with the lane
    // in the attribution.
    let linked = ToolContext {
        principal: Some(Principal::User {
            global_id: ALICE.into(),
        }),
        lane_key: Some("u_example_alice:telegram".into()),
        source: Some("telegram".into()),
        ..Default::default()
    };
    let ctx = h
        .service
        .context_for_tool(&linked, Capabilities::READ_ONLY)
        .unwrap();
    assert_eq!(ctx.owner_id, ALICE);
    assert_eq!(ctx.surface, Surface::ModelTool);
    assert_eq!(ctx.capabilities, Capabilities::READ_ONLY);
    assert_eq!(
        ctx.attribution.lane_key.as_deref(),
        Some("u_example_alice:telegram")
    );
    assert!(ctx.project_root.is_none());
    // The lead's `created_by` with no principal: the owner id, a row.
    let lead = ToolContext {
        owner_id: Some(ALICE.into()),
        task_id: Some("t-1".into()),
        request_workspace_root: Some(
            h.workspace("repo", Some(".git"))
                .to_string_lossy()
                .into_owned(),
        ),
        ..Default::default()
    };
    let ctx = h
        .service
        .context_for_tool(&lead, Capabilities::READ_ONLY)
        .unwrap();
    assert_eq!(
        ctx.project_root.as_deref(),
        Some(h.workspace("repo", Some(".git")).as_path())
    );
    assert_eq!(ctx.attribution.task_id.as_deref(), Some("t-1"));
}

// ============================================================================
// Partitions (design §3 rule 1, MD-5; fixtures 01–04)
// ============================================================================

/// Fixture 01: a request that names no workspace sees and writes only the
/// home partition, whatever directory the daemon happens to run in — the
/// service never reads the CWD, so a project partition with a note in it is
/// invisible to a connector lane, a scheduled skill and a bare API call.
#[test]
fn a_request_that_names_no_workspace_uses_the_global_partition_only() {
    let h = Harness::new();
    let project_a = h.workspace("project_a", Some(".openalpaca"));
    let in_project = h.user(ALICE, Some(&project_a));
    let theme = h.store(
        &h.user(ALICE, None),
        "Color theme preference",
        "# Color theme preference\n\nThe user prefers a dark color theme.\n",
        NoteKind::Preference,
    );
    let pnpm = h.store(
        &in_project,
        "Package manager for project A",
        "# Package manager for project A\n\nProject A installs with pnpm, never npm.\n",
        NoteKind::Procedure,
    );
    assert_eq!(pnpm.scope, NoteScope::Project);

    let connector = h.tool(ALICE, None);
    let scheduled = h
        .ctx(ALICE, None, Surface::ModelTool, Capabilities::READ_ONLY)
        .unwrap();
    let api = h.user(ALICE, None);
    assert_eq!(
        h.list_ids(&scheduled, ListInput::default()),
        HashSet::from([theme.id])
    );

    let fact = h.store(
        &connector,
        "Standup time",
        "# Standup time\n\nThe team standup is at 09:30.\n",
        NoteKind::Fact,
    );
    assert_eq!(
        fact.scope,
        NoteScope::Global,
        "a fact with no project lands in global"
    );
    assert_eq!(fact.origin, Origin::ExtractedUser);
    assert!(note_file_for(&h.home_notes_dir(ALICE_KEY), fact.id).is_some());
    let project_snapshot = h
        .snapshot(&StoreScope::Project(project_a.clone()), ALICE)
        .unwrap();
    assert!(!project_snapshot.notes.contains_key(&fact.id));
    assert!(project_snapshot.notes.contains_key(&pnpm.id));
    assert_eq!(
        h.list_ids(&api, ListInput::default()),
        HashSet::from([theme.id, fact.id])
    );
}

/// Fixture 02: a directory with no `.openalpaca` or `.git` above it is
/// global only; no partition is created there, and an explicit
/// `scope: project` is refused (§7b P1).
#[test]
fn a_directory_with_no_marker_is_global_only() {
    let h = Harness::new();
    let plain = h.workspace("plain_dir", None);
    let sub = plain.join("notes-about-nothing");
    fs::create_dir_all(&sub).unwrap();
    let shell = h.store(
        &h.user(ALICE, None),
        "Preferred shell",
        "# Preferred shell\n\nThe user prefers zsh with a minimal prompt.\n",
        NoteKind::Preference,
    );

    let in_plain = h.tool(ALICE, Some(&plain));
    assert!(in_plain.project_root.is_none());
    let in_sub = h.tool(ALICE, Some(&sub));
    let fact = h.store(
        &in_sub,
        "Meeting room",
        "# Meeting room\n\nThe weekly review is in room 4B.\n",
        NoteKind::Fact,
    );
    assert_eq!(fact.scope, NoteScope::Global);
    assert!(note_file_for(&h.home_notes_dir(ALICE_KEY), fact.id).is_some());
    assert!(!plain.join(".openalpaca").exists());
    assert!(!sub.join(".openalpaca").exists());

    let err = h
        .service
        .store(
            &h.user(ALICE, Some(&plain)),
            StoreInput {
                title: "Room again".into(),
                body: "# Room again\n\nStill room 4B.\n".into(),
                kind: Some(NoteKind::Fact),
                scope: Some(NoteScope::Project),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.class(), "invalid_input");
    assert!(err.to_string().contains("project"), "{err}");
    assert!(!plain.join(".openalpaca").exists());
    assert_eq!(
        h.list_ids(&in_plain, ListInput::default()),
        HashSet::from([shell.id, fact.id])
    );
}

/// Fixture 03: inside a git repository with no `.openalpaca/` yet, a read
/// creates nothing; the first project write lays the partition out with its
/// own `.gitignore`; the artifact path's `ensure_store` in between changes
/// nothing about the partition key; a preference still goes global.
#[test]
fn a_git_repo_gets_its_partition_lazily_before_and_after_the_first_artifact() {
    let h = Harness::new();
    let repo = h.workspace("repo", Some(".git"));
    let sub = repo.join("src").join("lib");
    fs::create_dir_all(&sub).unwrap();
    let ctx = h.tool(ALICE, Some(&sub));
    assert_eq!(ctx.project_root.as_deref(), Some(repo.as_path()));

    assert!(h.list_ids(&ctx, ListInput::default()).is_empty());
    assert!(
        !repo.join(".openalpaca").exists(),
        "a read never creates the project store"
    );

    let first = h
        .service
        .store(
            &ctx,
            StoreInput {
                title: "Test command".into(),
                body: "# Test command\n\nRun `cargo test -p openalpaca_storage` before pushing.\n"
                    .into(),
                kind: Some(NoteKind::Procedure),
                fact_key: Some("build.validation".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(first.scope, NoteScope::Project);
    assert_eq!(first.origin, Origin::ExtractedUser);
    let memory = repo.join(".openalpaca").join("memory");
    assert_eq!(
        fs::read_to_string(memory.join(".gitignore")).unwrap(),
        "*\n"
    );
    let notes = memory.join("owners").join(ALICE_KEY).join("notes");
    assert!(notes.is_dir());
    assert!(
        memory
            .join("owners")
            .join(ALICE_KEY)
            .join(".history")
            .is_dir()
    );
    let file = note_file_for(&notes, first.id).unwrap();
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.ends_with("--test-command.md"), "{name}");
    let snapshot = h
        .snapshot(&StoreScope::Project(repo.clone()), ALICE)
        .unwrap();
    assert!(
        snapshot
            .partition_key
            .starts_with(&format!("proj-{ALICE_KEY}-"))
    );
    assert_eq!(snapshot.owner_id, ALICE);
    assert!(snapshot.pending.is_none());
    assert_eq!(snapshot.notes[&first.id].origin, Origin::ExtractedUser);
    let key_before = snapshot.partition_key.clone();

    // The artifact path lays out the rest of `<repo>/.openalpaca/`.
    ensure_store(&StoreScope::Project(repo.clone())).unwrap();
    assert_eq!(
        fs::read_to_string(memory.join(".gitignore")).unwrap(),
        "*\n"
    );

    let second = h
        .service
        .store(
            &ctx,
            StoreInput {
                title: "Lint command".into(),
                body: "# Lint command\n\nRun `cargo clippy -p openalpaca_storage --all-targets`.\n"
                    .into(),
                kind: Some(NoteKind::Procedure),
                fact_key: Some("build.lint".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(second.scope, NoteScope::Project);
    let snapshot = h
        .snapshot(&StoreScope::Project(repo.clone()), ALICE)
        .unwrap();
    assert_eq!(
        snapshot.partition_key, key_before,
        "the same partition before and after"
    );
    assert!(snapshot.notes.contains_key(&first.id) && snapshot.notes.contains_key(&second.id));
    let control = h.home().join("state").join("memory-control");
    let project_snapshots = fs::read_dir(&control)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("proj-"))
        .count();
    assert_eq!(project_snapshots, 1);

    let pref = h.store(
        &ctx,
        "Diff style",
        "# Diff style\n\nThe user prefers unified diffs.\n",
        NoteKind::Preference,
    );
    assert_eq!(pref.scope, NoteScope::Global);
    assert!(note_file_for(&h.home_notes_dir(ALICE_KEY), pref.id).is_some());

    assert_eq!(
        h.list_ids(&ctx, ListInput::default()),
        HashSet::from([first.id, second.id, pref.id])
    );
    assert_eq!(
        h.list_ids(
            &ctx,
            ListInput {
                scope: Some(NoteScope::Project),
                ..Default::default()
            }
        ),
        HashSet::from([first.id, second.id])
    );
}

/// Fixture 04: a symlink at `.openalpaca` on the way to the partition
/// refuses the project partition; the request falls back to global, and an
/// explicit `scope: project` is then refused like any request with no
/// project.
#[cfg(unix)]
#[test]
fn a_symlink_on_the_partition_path_refuses_the_project_partition_and_falls_back_to_global() {
    let h = Harness::new();
    let repo = h.workspace("linked_repo", Some(".git"));
    let elsewhere = h.workspace("elsewhere", None);
    std::os::unix::fs::symlink(&elsewhere, repo.join(".openalpaca")).unwrap();
    let ctx = h.tool(ALICE, Some(&repo));
    assert_eq!(ctx.project_root.as_deref(), Some(repo.as_path()));

    let fact = h.store(
        &ctx,
        "Escape attempt",
        "# Escape attempt\n\nWould have followed the link.\n",
        NoteKind::Fact,
    );
    assert_eq!(fact.scope, NoteScope::Global);
    assert!(note_file_for(&h.home_notes_dir(ALICE_KEY), fact.id).is_some());
    assert!(
        !elsewhere.join("memory").exists(),
        "nothing lands where the link points"
    );

    let err = h
        .service
        .store(
            &ctx,
            StoreInput {
                title: "Explicit".into(),
                body: "x\n".into(),
                kind: Some(NoteKind::Fact),
                scope: Some(NoteScope::Project),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.class(), "invalid_input");
    assert_eq!(
        h.list_ids(&ctx, ListInput::default()),
        HashSet::from([fact.id])
    );
}

#[test]
fn the_home_store_itself_is_never_a_project() {
    let h = Harness::new();
    let home = h.home();
    let ctx = h
        .service
        .context(
            ALICE,
            WorkspaceHint::Root(&home),
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    assert!(ctx.project_root.is_none());
    let ctx = h
        .service
        .context(
            ALICE,
            WorkspaceHint::Path(&home),
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    assert!(ctx.project_root.is_none());
}

// ============================================================================
// Ownership on every read (fixtures 07, 09)
// ============================================================================

#[test]
fn read_and_list_never_cross_owners() {
    let h = Harness::new();
    let bobs_project = h.workspace("bobs_project", Some(".openalpaca"));
    let bob_note = h.store(
        &h.user(BOB, None),
        "Bob's deploy notes location",
        "# Bob's deploy notes location\n\nBob keeps his deploy notes in the vault folder.\n",
        NoteKind::Fact,
    );
    let bob_project_note = h.store(
        &h.user(BOB, Some(&bobs_project)),
        "Bob's project deploy step",
        "# Bob's project deploy step\n\nDeploy with the release script.\n",
        NoteKind::Procedure,
    );
    let alice_note = h.store(
        &h.user(ALICE, None),
        "Alice's editor",
        "# Alice's editor\n\nAlice prefers vim.\n",
        NoteKind::Preference,
    );

    let alice = h.tool(ALICE, None);
    let err = h
        .service
        .read(
            &alice,
            ReadInput {
                id: bob_note.id,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.class(), "not_found");
    let text = err.to_string();
    assert!(
        !text.contains("vault folder") && !text.contains("Bob"),
        "{text}"
    );
    assert_eq!(
        h.list_ids(&alice, ListInput::default()),
        HashSet::from([alice_note.id])
    );

    let bob = h.tool(BOB, Some(&bobs_project));
    assert_eq!(
        h.list_ids(&bob, ListInput::default()),
        HashSet::from([bob_note.id, bob_project_note.id])
    );
    let read = h
        .service
        .read(
            &bob,
            ReadInput {
                id: bob_project_note.id,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(read.scope, NoteScope::Project);
    assert_eq!(read.origin, Origin::User);
    assert_eq!(read.freshness, Freshness::Verified);
    assert!(read.path.starts_with("notes/"));
}

/// Fixture 09 steps 4 and 6: exact dedup is within one owner and one
/// partition — the same sentence in Alice's project is a new note, the same
/// sentence Bob already holds answers with Bob's existing id.
#[test]
fn exact_dedup_is_within_one_partition_only() {
    let h = Harness::new();
    let project_a = h.workspace("project_a", Some(".openalpaca"));
    let body = "# Indentation preference\n\nPrefers tabs over spaces for indentation.\n";
    let a1 = h.store(
        &h.user(ALICE, None),
        "Indentation preference",
        body,
        NoteKind::Preference,
    );
    let b1 = h.store(
        &h.user(BOB, None),
        "Indentation preference",
        body,
        NoteKind::Preference,
    );

    let again = h.store(
        &h.tool(ALICE, Some(&project_a)),
        "Indentation preference",
        body,
        NoteKind::Decision,
    );
    assert_eq!(again.dedup, Dedup::Created);
    assert_eq!(again.scope, NoteScope::Project);
    assert_ne!(again.id, a1.id);

    let bob_again = h.store(
        &h.tool(BOB, None),
        "Indentation preference",
        body,
        NoteKind::Preference,
    );
    assert_eq!(bob_again.dedup, Dedup::Existing);
    assert_eq!(bob_again.id, b1.id);
    assert!(bob_again.change.is_none(), "a dedup publishes nothing");
}

// ============================================================================
// Capabilities, validation (design §7a; fixtures 14, 20)
// ============================================================================

#[test]
fn a_write_needs_the_write_capability_and_a_read_the_read_one() {
    let h = Harness::new();
    let worker = h
        .ctx(ALICE, None, Surface::ModelTool, Capabilities::READ_ONLY)
        .unwrap();
    let err = h
        .service
        .store(
            &worker,
            StoreInput {
                title: "Worker write".into(),
                body: "# Worker write\n\nA worker may not write.\n".into(),
                kind: Some(NoteKind::Fact),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.class(), "fixture:capability_denied");
    assert!(err.to_string().contains("memory_write"), "{err}");
    assert!(
        !h.home_notes_dir(ALICE_KEY).exists(),
        "nothing was laid out"
    );

    let extractor = h
        .ctx(ALICE, None, Surface::Extraction, Capabilities::EXTRACTION)
        .unwrap();
    let err = h
        .service
        .list(&extractor, ListInput::default())
        .unwrap_err();
    assert_eq!(err.class(), "fixture:capability_denied");
    assert!(err.to_string().contains("memory_read"), "{err}");
}

#[test]
fn sources_must_exist_in_the_database() {
    let h = Harness::new();
    let ctx = h.tool(ALICE, None);
    let missing = h
        .service
        .store(
            &ctx,
            StoreInput {
                title: "Cited".into(),
                body: "# Cited\n\nFrom a message.\n".into(),
                kind: Some(NoteKind::Fact),
                sources: vec![NoteSource {
                    kind: SourceType::Message,
                    id: "999999".into(),
                    observed_at: None,
                }],
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(missing.class(), "invalid_input");
    assert!(missing.to_string().contains("message:999999"), "{missing}");

    let rowid = h.message_row("I prefer vim");
    let stored = h
        .service
        .store(
            &ctx,
            StoreInput {
                title: "Cited".into(),
                body: "# Cited\n\nFrom a message.\n".into(),
                kind: Some(NoteKind::Fact),
                sources: vec![NoteSource {
                    kind: SourceType::Message,
                    id: rowid.to_string(),
                    observed_at: None,
                }],
                ..Default::default()
            },
        )
        .unwrap();
    let read = h
        .service
        .read(
            &ctx,
            ReadInput {
                id: stored.id,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(read.sources.len(), 1);
    assert_eq!(read.sources[0].id, rowid.to_string());
}

/// Fixture 14 steps 6–8: 32 KiB + 1 byte is refused before anything is
/// written, a note at the limit is accepted and read back whole. (The
/// fixture sizes its body for a fatter frontmatter; the bound is the whole
/// file's, so the body here is over the limit on its own.)
#[test]
fn oversized_and_invalid_inputs_are_refused_before_anything_is_written() {
    let h = Harness::new();
    let ctx = h.tool(ALICE, None);
    let unit = "lorem ipsum dolor sit amet ";
    let too_large: String = unit.repeat(32769 / unit.len() + 1)[..32769].to_string();
    let err = h
        .service
        .store(
            &ctx,
            StoreInput {
                title: "Too large".into(),
                body: too_large,
                kind: Some(NoteKind::Reference),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.class(), "invalid_input");
    assert!(err.to_string().contains("32"), "{err}");
    let snapshot = h.snapshot(&StoreScope::Home, ALICE);
    assert!(
        snapshot
            .as_ref()
            .is_none_or(|s| s.pending.is_none() && s.notes.is_empty())
    );

    let at_limit: String = unit.repeat(32100 / unit.len() + 1)[..32100].to_string();
    let stored = h
        .service
        .store(
            &ctx,
            StoreInput {
                title: "At the limit".into(),
                body: at_limit.clone(),
                kind: Some(NoteKind::Reference),
                ..Default::default()
            },
        )
        .unwrap();
    let read = h
        .service
        .read(
            &ctx,
            ReadInput {
                id: stored.id,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!read.truncated);
    assert!(read.body.len() <= 32768);
    assert_eq!(read.body, at_limit);

    for (what, input) in [
        (
            "fact_key grammar",
            StoreInput {
                title: "k".into(),
                body: "x\n".into(),
                kind: Some(NoteKind::Fact),
                fact_key: Some("Not.Valid".into()),
                ..Default::default()
            },
        ),
        (
            "title length",
            StoreInput {
                title: "t".repeat(201),
                body: "x\n".into(),
                kind: Some(NoteKind::Fact),
                ..Default::default()
            },
        ),
        (
            "alias count",
            StoreInput {
                title: "a".into(),
                body: "x\n".into(),
                kind: Some(NoteKind::Fact),
                aliases: (0..17).map(|i| format!("a{i}")).collect(),
                ..Default::default()
            },
        ),
        (
            "missing kind",
            StoreInput {
                title: "a".into(),
                body: "x\n".into(),
                ..Default::default()
            },
        ),
    ] {
        let err = h.service.store(&ctx, input).unwrap_err();
        assert_eq!(err.class(), "invalid_input", "{what}: {err}");
    }
}

/// Fixture 14 step 1: two saves at once against one partition — the
/// per-partition writer queue lands both.
#[test]
fn two_concurrent_stores_on_one_partition_both_land() {
    let h = Harness::new();
    let service = &h.service;
    let one = h.tool(ALICE, None);
    let two = h.tool(ALICE, None);
    let (a, b) = std::thread::scope(|s| {
        let ta = s.spawn(|| {
            service
                .store(
                    &one,
                    StoreInput {
                        title: "Save one".into(),
                        body: "# Save one\n\nFirst concurrent save.\n".into(),
                        kind: Some(NoteKind::Fact),
                        ..Default::default()
                    },
                )
                .unwrap()
        });
        let tb = s.spawn(|| {
            service
                .store(
                    &two,
                    StoreInput {
                        title: "Save two".into(),
                        body: "# Save two\n\nSecond concurrent save.\n".into(),
                        kind: Some(NoteKind::Fact),
                        ..Default::default()
                    },
                )
                .unwrap()
        });
        (ta.join().unwrap(), tb.join().unwrap())
    });
    let notes = h.home_notes_dir(ALICE_KEY);
    assert!(note_file_for(&notes, a.id).is_some());
    assert!(note_file_for(&notes, b.id).is_some());
    let snapshot = h.snapshot(&StoreScope::Home, ALICE).unwrap();
    assert!(snapshot.pending.is_none());
    assert_eq!(snapshot.notes[&a.id].origin, Origin::ExtractedUser);
    assert_eq!(snapshot.notes[&b.id].origin, Origin::ExtractedUser);
    assert_eq!(
        h.list_ids(&one, ListInput::default()),
        HashSet::from([a.id, b.id])
    );
}

#[test]
fn quiesce_refuses_while_a_write_is_in_flight_and_holds_new_writes() {
    let slots = write_slot::WriteSlots::default();
    let key = "proj:/tmp/x";
    let writing = slots.begin_write(key);
    assert!(
        slots.quiesce(key).is_err(),
        "a pending write refuses the move"
    );
    drop(writing);
    let held = slots.quiesce(key).expect("idle: the move may proceed");
    assert!(slots.is_busy(key));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|s| {
        s.spawn(|| {
            let _w = slots.begin_write(key);
            tx.send(()).unwrap();
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(150))
                .is_err(),
            "a new write waits while the move holds the slot"
        );
        drop(held);
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("the held writer proceeds once the move releases");
    });
    assert!(!slots.is_busy(key));
}

#[test]
fn the_service_quiesce_is_keyed_by_project_root() {
    let h = Harness::new();
    let repo = h.workspace("repo", Some(".git"));
    let guard = h.service.quiesce(&repo).unwrap();
    let err = h.service.quiesce(&repo).unwrap_err();
    assert_eq!(err.class(), "fixture:write_pending");
    drop(guard);
    h.service.quiesce(&repo).unwrap();
}

// ============================================================================
// Extraction writes (design §7a, MD-2)
// ============================================================================

#[test]
fn an_extraction_write_records_by_provenance() {
    let h = Harness::new();
    let extractor = h
        .ctx(ALICE, None, Surface::Extraction, Capabilities::EXTRACTION)
        .unwrap();
    let said = h
        .service
        .store(
            &extractor,
            StoreInput {
                title: "Prefers vim".into(),
                body: "# Prefers vim\n\nThe user prefers vim.\n".into(),
                kind: Some(NoteKind::Preference),
                provenance: Some(ExtractionProvenance::UserWords),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(said.origin, Origin::ExtractedUser);
    let claimed = h
        .service
        .store(
            &extractor,
            StoreInput {
                title: "Staging URL changed".into(),
                body: "# Staging URL changed\n\nAn MCP tool claimed the staging URL changed.\n"
                    .into(),
                kind: Some(NoteKind::Fact),
                provenance: Some(ExtractionProvenance::OtherOutput),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(claimed.origin, Origin::ExtractedOther);
    let reader = h.user(ALICE, None);
    let active = h.list_ids(&reader, ListInput::default());
    assert_eq!(
        active,
        HashSet::from([said.id]),
        "a candidate is not active"
    );
    let candidates = h.list_ids(
        &reader,
        ListInput {
            include_candidates: true,
            ..Default::default()
        },
    );
    assert_eq!(candidates, HashSet::from([said.id, claimed.id]));

    let err = h
        .service
        .store(
            &extractor,
            StoreInput {
                title: "No provenance".into(),
                body: "x\n".into(),
                kind: Some(NoteKind::Fact),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.class(), "invalid_input");
    let err = h
        .service
        .store(
            &h.tool(ALICE, None),
            StoreInput {
                title: "Not the extractor".into(),
                body: "x\n".into(),
                kind: Some(NoteKind::Fact),
                provenance: Some(ExtractionProvenance::UserWords),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.class(), "invalid_input");
}

// ============================================================================
// list / read / recently learned / the ring
// ============================================================================

#[test]
fn list_paginates_with_a_cursor_bound_to_its_filters() {
    let h = Harness::new();
    let ctx = h.user(ALICE, None);
    let mut ids = HashSet::new();
    for i in 0..3 {
        ids.insert(
            h.store(
                &ctx,
                &format!("Note {i}"),
                &format!("# Note {i}\n\nBody {i}.\n"),
                NoteKind::Fact,
            )
            .id,
        );
    }
    let page1 = h
        .service
        .list(
            &ctx,
            ListInput {
                limit: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(page1.items.len(), 2);
    let cursor = page1.next_cursor.clone().expect("a third note remains");
    let page2 = h
        .service
        .list(
            &ctx,
            ListInput {
                limit: Some(2),
                cursor: Some(cursor.clone()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(page2.items.len(), 1);
    assert!(page2.next_cursor.is_none());
    let mut seen: HashSet<NoteId> = page1.items.iter().map(|i| i.id).collect();
    seen.extend(page2.items.iter().map(|i| i.id));
    assert_eq!(seen, ids);

    let err = h
        .service
        .list(
            &ctx,
            ListInput {
                limit: Some(2),
                cursor: Some(cursor),
                kind: Some(NoteKind::Preference),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(
        err.class(),
        "invalid_input",
        "a cursor from another filter set"
    );
    assert!(
        h.service
            .list(
                &ctx,
                ListInput {
                    limit: Some(101),
                    ..Default::default()
                }
            )
            .is_err()
    );
}

#[test]
fn read_returns_a_section_and_the_frontmatter() {
    let h = Harness::new();
    let ctx = h.user(ALICE, None);
    let stored = h
        .service
        .store(
            &ctx,
            StoreInput {
                title: "Build and test".into(),
                body: "# Build and test\n\n## Knowledge\nRun the focused checks.\n\n## Evidence and limits\nOne component only.\n\n## Recheck when\nThe build changes.\n".into(),
                kind: Some(NoteKind::Procedure),
                aliases: vec!["checks".into(), "测试".into()],
                fact_key: Some("build.validation".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let whole = h
        .service
        .read(
            &ctx,
            ReadInput {
                id: stored.id,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        whole.frontmatter["aliases"],
        serde_json::json!(["checks", "测试"])
    );
    assert_eq!(
        whole.frontmatter["fact_key"],
        serde_json::json!("build.validation")
    );
    assert_eq!(whole.frontmatter["kind"], serde_json::json!("procedure"));
    assert_eq!(whole.revision, stored.revision);
    let section = h
        .service
        .read(
            &ctx,
            ReadInput {
                id: stored.id,
                section: Some("Evidence and limits".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        section.body,
        "## Evidence and limits\nOne component only.\n"
    );
    let err = h
        .service
        .read(
            &ctx,
            ReadInput {
                id: stored.id,
                expected_revision: Some("f".repeat(64)),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.class(), "revision_conflict");
}

#[test]
fn recently_learned_lists_extracted_user_notes_newest_first() {
    let h = Harness::new();
    let tool = h.tool(ALICE, None);
    let a = h.store(
        &tool,
        "Learned one",
        "# Learned one\n\nOne.\n",
        NoteKind::Fact,
    );
    let b = h.store(
        &tool,
        "Learned two",
        "# Learned two\n\nTwo.\n",
        NoteKind::Fact,
    );
    let explicit = h.store(
        &h.user(ALICE, None),
        "Saved by hand",
        "# Saved by hand\n\nMine.\n",
        NoteKind::Fact,
    );
    let learned: Vec<NoteId> = h
        .service
        .recently_learned(&h.user(ALICE, None), None, None)
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(learned.len(), 2);
    assert!(learned.contains(&a.id) && learned.contains(&b.id));
    assert!(!learned.contains(&explicit.id));
}

#[test]
fn the_recently_used_ring_is_bounded_and_newest_first() {
    let h = Harness::new();
    let tool = h.tool(ALICE, None);
    let note = h.store(&tool, "Used", "# Used\n\nRead me.\n", NoteKind::Fact);
    assert!(
        h.service.recently_used(50).is_empty(),
        "a store is not a use"
    );
    h.service
        .read(
            &tool,
            ReadInput {
                id: note.id,
                ..Default::default()
            },
        )
        .unwrap();
    let ring = h.service.recently_used(50);
    assert_eq!(ring.len(), 1);
    assert_eq!(ring[0].id, note.id);
    assert_eq!(ring[0].reason, UsedReason::Tool);
    // A user-surface read is not a delivery to a model.
    h.service
        .read(
            &h.user(ALICE, None),
            ReadInput {
                id: note.id,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(h.service.recently_used(50).len(), 1);
    for _ in 0..RING_CAPACITY + 5 {
        h.service.record_used(
            &tool,
            &[MemoryUsed {
                id: note.id,
                revision: note.revision.clone(),
                reason: UsedReason::Recall,
            }],
        );
    }
    assert_eq!(h.service.recently_used(500).len(), RING_CAPACITY);
    assert_eq!(h.service.recently_used(3).len(), 3);
}
