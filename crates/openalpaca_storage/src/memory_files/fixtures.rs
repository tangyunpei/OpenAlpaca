//! The invariant-fixture loader for the file layer
//! (`crates/openalpaca_storage/tests/fixtures/memory/README.md`).
//!
//! Slice 2 has no service, so a service-level op is the library call
//! underneath: `store` / `update` / `forget` / `read` / `list` are
//! `memory_files` mutations and reads on the partition the context resolves
//! to, with the context's `surface` mapped to the origin it records. The
//! index-level ops (`search`, `recall`) only *open* the context's partitions
//! here — which is what makes "a read never creates the project store" and
//! the symlink fallback observable — and their result expectations are the
//! `memory_index` loader's. Ops slice 2 cannot run are skipped by name.
//!
//! The loader never computes a revision or a key itself: every `@current`,
//! `@rev:` and `@derived` is read back through the library.

use super::*;
use crate::store::tests::HomeStoreGuard;
use crate::store::{ContentKind, StoreScope, content_dir};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tempfile::{TempDir, tempdir};

/// The fixtures whose file-level operations this loader satisfies.
const FILE_LEVEL_FIXTURES: &[&str] = &[
    "03-git-repo-lazy-partition.json",
    "04-symlink-on-partition-path-refused.json",
    "09-same-phrase-two-owners.json",
    "10-authority-order-correction.json",
    "13-read-after-write-and-freshness.json",
    "14-concurrent-saves-conflict-large-note-duplicate-uuid.json",
    "15-crash-at-each-recovery-step.json",
    "16-forget-suppression-and-replacement.json",
    "18-malicious-content-stays-reference-data.json",
    "24-forgotten-replacement-not-revived.json",
    "25-suggestion-lifecycle.json",
];

/// Ops another layer owns; a step with one of these is skipped whole.
const SKIPPED_OPS: &[&str] = &[
    "tool_call",
    "extraction",
    "recently_used",
    "begin_pending_write",
    "end_pending_write",
    "move_workspace",
    "purge",
    "factory_reset",
    "config_reset",
    "boot",
    "open_database",
    "erase_cache",
];

/// Expectation keys the index or the service answers, never the file layer.
const OTHER_LAYER_KEYS: &[&str] = &[
    "events",
    "events_count",
    "index_state",
    "memory_used",
    "memory_used_includes",
    "not_served_from_cache",
    "partial",
    "retained",
    "delivered_as",
    "trust_label_present",
    "provenance_fields",
    "total_bytes_lte",
    "chunks_lte",
    "pinned_bytes_lte",
    "network_calls",
    "model_calls",
    "items_include",
    "preview_contains",
    "owner_id_after",
    "extraction",
    "ids_may_include",
    "ids_absent_by_status",
];

/// The fixture author budgeted 600 bytes of frontmatter when sizing fixture
/// 14's `@repeat` bodies ("the `bytes` values leave 600 bytes for the
/// frontmatter") and told the loader to size the body so the whole file lands
/// on the intended side of 32 KiB. The canon's frontmatter for those notes is
/// 317 bytes, so the body is grown by the difference: the whole file is then
/// exactly `bytes + 600`, which is what the author meant.
const FIXTURE_FRONTMATTER_BUDGET: usize = 600;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("memory")
}

#[derive(Clone)]
struct PartitionSpec {
    owner: String,
    scope: StoreScope,
    lazy: bool,
}

#[derive(Clone)]
struct Context {
    owner_id: String,
    workspace: Option<PathBuf>,
    actor: Actor,
}

struct Harness {
    _tmp: TempDir,
    _env: HomeStoreGuard,
    home: PathBuf,
    ws_root: PathBuf,
    dir_root: PathBuf,
    local_user_id: String,
    partitions: BTreeMap<String, PartitionSpec>,
    contexts: HashMap<String, Context>,
    /// Open partitions by name; dropped and reopened on `restart`.
    opened: HashMap<String, Partition>,
    labels: HashMap<String, NoteId>,
    /// `@from_rev:<label>`: the revision a note had before the labelled step.
    pre_revisions: HashMap<String, String>,
    /// The interrupted mutation's `(from_rev, to_rev)`.
    last_crash: Option<(Option<String>, String)>,
    warnings: Vec<String>,
    clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

fn ticking() -> Arc<dyn Fn() -> DateTime<Utc> + Send + Sync> {
    let counter = Arc::new(AtomicI64::new(0));
    let base = DateTime::parse_from_rfc3339("2026-09-23T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    Arc::new(move || base + chrono::Duration::seconds(counter.fetch_add(1, Ordering::SeqCst)))
}

fn s(v: &Value) -> &str {
    v.as_str()
        .unwrap_or_else(|| panic!("expected a string, got {v}"))
}

fn arr(v: &Value) -> &Vec<Value> {
    v.as_array()
        .unwrap_or_else(|| panic!("expected an array, got {v}"))
}

fn obj(v: &Value) -> &Map<String, Value> {
    v.as_object()
        .unwrap_or_else(|| panic!("expected an object, got {v}"))
}

fn parse_time(v: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(v).unwrap().with_timezone(&Utc)
}

/// The project root a request workspace resolves to: the store's own
/// resolver (`store::projects::resolve`, storage design §3) — the nearest
/// registered project or folder holding `.git`, never the home store. A
/// directory under neither is global.
fn find_project_root(from: &Path) -> Option<PathBuf> {
    crate::store::projects::resolve(from)
        .unwrap_or_else(|e| panic!("resolve {}: {e:#}", from.display()))
        .map(|found| found.root)
}

impl Harness {
    fn build(fixture: &Value) -> Self {
        let tmp = tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let env = HomeStoreGuard::set(&home);
        let ws_root = root.join("ws");
        let dir_root = root.join("dir");
        std::fs::create_dir_all(&ws_root).unwrap();
        std::fs::create_dir_all(&dir_root).unwrap();

        for owner in arr(&fixture["owners"]) {
            assert_eq!(
                owner_key(s(&owner["id"])),
                s(&owner["owner_key"]),
                "the fixture's owner key derivation"
            );
        }
        let local_user_id = fixture["database"]["local_user_id"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let mut h = Self {
            _tmp: tmp,
            _env: env,
            home,
            ws_root,
            dir_root,
            local_user_id,
            partitions: BTreeMap::new(),
            contexts: HashMap::new(),
            opened: HashMap::new(),
            labels: HashMap::new(),
            pre_revisions: HashMap::new(),
            last_crash: None,
            warnings: Vec::new(),
            clock: ticking(),
        };
        for name in fixture["directories"].as_array().into_iter().flatten() {
            std::fs::create_dir_all(h.dir_root.join(s(name))).unwrap();
        }
        for ws in fixture["workspaces"].as_array().into_iter().flatten() {
            let dir = h.ws_root.join(s(&ws["name"]));
            std::fs::create_dir_all(&dir).unwrap();
            match ws["marker"].as_str() {
                Some(".git") => std::fs::create_dir_all(dir.join(".git")).unwrap(),
                None => {}
                Some(other) => panic!(
                    "workspace {}: marker {other:?} — a fixture's marker is \".git\" or null \
                     (`.openalpaca` is no project marker, storage design §3)",
                    s(&ws["name"])
                ),
            }
            for sub in ws["subdirs"].as_array().into_iter().flatten() {
                std::fs::create_dir_all(dir.join(s(sub))).unwrap();
            }
            for link in ws["symlinks"].as_array().into_iter().flatten() {
                let target = h.resolve_input_path(s(&link["target"]));
                #[cfg(unix)]
                std::os::unix::fs::symlink(&target, dir.join(s(&link["path"]))).unwrap();
                #[cfg(not(unix))]
                let _ = target;
            }
            for file in ws["files"].as_array().into_iter().flatten() {
                std::fs::write(dir.join(s(&file["path"])), s(&file["content"])).unwrap();
            }
        }
        for p in fixture["partitions"].as_array().into_iter().flatten() {
            let name = s(&p["name"]).to_string();
            let scope = match s(&p["scope"]) {
                "home" => StoreScope::Home,
                "project" => {
                    // A project partition is a registered project's (design
                    // §3, R12): the loader registers the workspace the way the
                    // daemon's first turn there would.
                    let ws = h.ws_root.join(s(&p["workspace"]));
                    crate::store::projects::register(&ws).unwrap();
                    StoreScope::Project(ws)
                }
                other => panic!("scope {other}"),
            };
            let spec = PartitionSpec {
                owner: s(&p["owner"]).to_string(),
                scope,
                lazy: p["lazy"].as_bool().unwrap_or(false),
            };
            if !spec.lazy {
                h.seed_partition(&spec, p);
            }
            h.partitions.insert(name, spec);
        }
        // Links and files inside a project's folder under the home store
        // (fixture 04): made after registration, which decides the folder's
        // name a `proj:` path needs.
        for link in fixture["home_links"].as_array().into_iter().flatten() {
            let path = h.resolve_path(s(&link["path"]));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let target = h.resolve_input_path(s(&link["target"]));
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &path).unwrap();
            #[cfg(not(unix))]
            let _ = target;
        }
        for file in fixture["home_files"].as_array().into_iter().flatten() {
            let path = h.resolve_path(s(&file["path"]));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, s(&file["content"])).unwrap();
        }
        for (name, c) in fixture["contexts"].as_object().into_iter().flatten() {
            let owner_id = c["owner_id"]
                .as_str()
                .unwrap_or_else(|| panic!("context {name}: a null owner is a slice-3 case"))
                .to_string();
            let workspace = c["workspace"].as_str().map(|w| h.resolve_path(w));
            let actor = match s(&c["surface"]) {
                "user_surface" => Actor::UserSurface,
                "model_tool" => Actor::ModelTool,
                // The extractor records what it was told (design §7a): the
                // user's own words unless the context names another origin.
                "extraction" => Actor::Extraction {
                    origin: c["origin"]
                        .as_str()
                        .map(|o| Origin::parse(o).unwrap_or_else(|| panic!("origin {o}")))
                        .unwrap_or(Origin::ExtractedUser),
                },
                other => panic!("surface {other}"),
            };
            h.contexts.insert(
                name.clone(),
                Context {
                    owner_id,
                    workspace,
                    actor,
                },
            );
        }
        h.open_all();
        h
    }

    // ------------------------------------------------------------------------
    // Paths
    // ------------------------------------------------------------------------

    /// `home`, `ws:<name>[/rest]`, `dir:<name>[/rest]`, `part:<name>[/rest]`,
    /// `proj:<name>[/rest]` (workspace `<name>`'s folder under `projects/`).
    fn resolve_path(&self, fixture_path: &str) -> PathBuf {
        if fixture_path == "home" {
            return self.home.clone();
        }
        if let Some(rest) = fixture_path.strip_prefix("home/") {
            return self.home.join(rest);
        }
        let (root, rest) = fixture_path
            .split_once(':')
            .unwrap_or_else(|| panic!("fixture path {fixture_path}"));
        let (name, tail) = rest.split_once('/').unwrap_or((rest, ""));
        let base = match root {
            "ws" => self.ws_root.join(name),
            "dir" => self.dir_root.join(name),
            "part" => self.partition_root(name),
            "proj" => {
                let project = crate::store::projects::lookup(&self.ws_root.join(name))
                    .unwrap()
                    .unwrap_or_else(|| {
                        panic!(
                            "fixture path proj:{name}: the workspace is not a registered project"
                        )
                    });
                crate::store::projects::projects_dir()
                    .unwrap()
                    .join(project.name)
            }
            other => panic!("fixture path root {other}"),
        };
        if tail.is_empty() {
            base
        } else {
            base.join(tail)
        }
    }

    /// `@ws:<name>` / `@dir:<name>` / `@part:<name>` in inputs.
    fn resolve_input_path(&self, v: &str) -> PathBuf {
        self.resolve_path(v.strip_prefix('@').unwrap_or(v))
    }

    fn partition_root(&self, name: &str) -> PathBuf {
        let spec = &self.partitions[name];
        let store = match &spec.scope {
            StoreScope::Home => self.home.clone(),
            StoreScope::Project(p) => {
                let project = crate::store::projects::lookup(p)
                    .unwrap()
                    .expect("a project partition's workspace is registered");
                crate::store::projects::projects_dir()
                    .unwrap()
                    .join(project.name)
            }
        };
        store
            .join("memory")
            .join("owners")
            .join(owner_key(&spec.owner))
    }

    // ------------------------------------------------------------------------
    // Seeding
    // ------------------------------------------------------------------------

    fn seed_partition(&mut self, spec: &PartitionSpec, p: &Value) {
        let partition = Partition::open(&spec.scope, &spec.owner).unwrap();
        partition.initialize().unwrap();
        let mut snapshot = partition.snapshot().unwrap();
        let clock = p["clock"].as_str().unwrap_or("2026-09-23T10:00:00Z");
        snapshot.created_at = parse_time(clock);
        for n in p["notes"].as_array().into_iter().flatten() {
            let (id, bytes, file_name, updated_at) = note_bytes(n);
            std::fs::write(partition.notes_dir().join(&file_name), &bytes).unwrap();
            if n["recorded"].as_bool().unwrap_or(true) {
                snapshot.notes.insert(
                    id,
                    NoteRecord {
                        revision: revision_of(&bytes),
                        origin: Origin::parse(s(&n["origin"])).unwrap(),
                        written_at: updated_at.unwrap_or_else(|| parse_time(clock)),
                    },
                );
            }
        }
        let snapshot_path = partition.snapshot_path().to_path_buf();
        drop(partition);
        match &p["snapshot"] {
            Value::Null => snapshot.save(&snapshot_path, parse_time(clock)).unwrap(),
            Value::String(m) if m == "missing" => std::fs::remove_file(&snapshot_path).unwrap(),
            Value::Object(o) if o.contains_key("raw") => {
                std::fs::write(&snapshot_path, s(&o["raw"])).unwrap();
            }
            Value::Object(o) => {
                if let Some(notes) = o.get("notes") {
                    for (id, rec) in obj(notes) {
                        let id = NoteId::parse(id).unwrap();
                        if rec.is_null() {
                            snapshot.notes.remove(&id);
                            continue;
                        }
                        let current = snapshot.notes.get(&id).cloned();
                        let revision = match s(&rec["revision"]) {
                            "@current" => current
                                .as_ref()
                                .map(|r| r.revision.clone())
                                .unwrap_or_else(|| {
                                    let path = self
                                        .file_for_in(&snapshot_root_notes(&snapshot), &id)
                                        .expect("a file for @current");
                                    revision_of(&std::fs::read(path).unwrap())
                                }),
                            literal => literal.to_string(),
                        };
                        snapshot.notes.insert(
                            id,
                            NoteRecord {
                                revision,
                                origin: Origin::parse(s(&rec["origin"])).unwrap(),
                                written_at: rec["written_at"]
                                    .as_str()
                                    .map(parse_time)
                                    .or_else(|| current.map(|r| r.written_at))
                                    .unwrap_or_else(|| parse_time(clock)),
                            },
                        );
                    }
                }
                if let Some(t) = o.get("tombstones") {
                    snapshot.tombstones = arr(t).iter().map(tombstone_from).collect();
                }
                if let Some(pending) = o.get("pending") {
                    snapshot.pending = if pending.is_null() {
                        None
                    } else {
                        Some(Pending {
                            id: NoteId::parse(s(&pending["id"])).unwrap(),
                            from_rev: pending["from_rev"].as_str().map(str::to_string),
                            to_rev: s(&pending["to_rev"]).to_string(),
                            origin: Origin::parse(pending["origin"].as_str().unwrap_or("user"))
                                .unwrap(),
                        })
                    };
                }
                if let Some(m) = o.get("moved_from") {
                    snapshot.moved_from = m.as_str().map(str::to_string);
                }
                if let Some(r) = o.get("repaired_at") {
                    snapshot.repaired_at = r.as_str().map(parse_time);
                }
                snapshot.save(&snapshot_path, parse_time(clock)).unwrap();
            }
            other => panic!("snapshot spec {other}"),
        }
    }

    fn open_all(&mut self) {
        self.opened.clear();
        let names: Vec<String> = self.partitions.keys().cloned().collect();
        for name in names {
            let spec = self.partitions[&name].clone();
            if spec.lazy && !self.partition_root(&name).exists() {
                continue;
            }
            match Partition::open(&spec.scope, &spec.owner) {
                Ok(p) => {
                    let clock = Arc::clone(&self.clock);
                    self.opened.insert(name, p.with_clock(move || clock()));
                }
                Err(MemoryFilesError::PathRefused { .. }) => {}
                Err(e) => panic!("open {name}: {e}"),
            }
        }
    }

    // ------------------------------------------------------------------------
    // Contexts → partitions
    // ------------------------------------------------------------------------

    fn partition_name_for(&self, owner: &str, scope: &StoreScope) -> Option<String> {
        self.partitions
            .iter()
            .find(|(_, spec)| spec.owner == owner && spec.scope == *scope)
            .map(|(name, _)| name.clone())
    }

    /// The home partition name of a context's owner.
    fn home_partition_of(&self, ctx: &Context) -> String {
        self.partition_name_for(&ctx.owner_id, &StoreScope::Home)
            .unwrap_or_else(|| panic!("no home partition listed for {}", ctx.owner_id))
    }

    /// The project partition name a context resolves to, opening it (a
    /// refusal falls back to global with one warning, as the service does).
    fn project_partition_of(&mut self, ctx: &Context) -> Option<String> {
        let root = ctx.workspace.as_deref().and_then(find_project_root)?;
        let scope = StoreScope::Project(root);
        let name = self.partition_name_for(&ctx.owner_id, &scope)?;
        if !self.opened.contains_key(&name) {
            let spec = self.partitions[&name].clone();
            match Partition::open(&spec.scope, &spec.owner) {
                Ok(p) => {
                    let clock = Arc::clone(&self.clock);
                    self.opened
                        .insert(name.clone(), p.with_clock(move || clock()));
                }
                Err(e @ MemoryFilesError::PathRefused { .. }) => {
                    self.warnings.push(e.to_string());
                    return None;
                }
                Err(e) => panic!("open {name}: {e}"),
            }
        }
        Some(name)
    }

    /// Every partition the context reads: global, plus the project when there
    /// is one.
    fn read_partitions_of(&mut self, ctx: &Context) -> Vec<String> {
        let mut names = vec![self.home_partition_of(ctx)];
        if let Some(p) = self.project_partition_of(ctx) {
            names.push(p);
        }
        names
    }

    fn write_partition_of(&mut self, ctx: &Context, kind: NoteKind, scope: Option<&str>) -> String {
        let project = self.project_partition_of(ctx);
        let wanted = match scope {
            Some("global") => NoteScope::Global,
            Some("project") => NoteScope::Project,
            _ => kind.default_scope(),
        };
        match (wanted, project) {
            (NoteScope::Project, Some(name)) => name,
            _ => self.home_partition_of(ctx),
        }
    }

    fn partition(&self, name: &str) -> &Partition {
        self.opened
            .get(name)
            .unwrap_or_else(|| panic!("partition {name} is not open"))
    }

    /// Where a note lives, by id, among the open partitions.
    fn locate(&self, id: &NoteId) -> Option<(String, NoteView)> {
        for (name, p) in &self.opened {
            if let Ok(view) = p.read(id) {
                return Some((name.clone(), view));
            }
        }
        None
    }

    fn file_for_in(&self, notes_dir: &Path, id: &NoteId) -> Option<PathBuf> {
        let prefix = format!("{id}--");
        std::fs::read_dir(notes_dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with(&prefix))
                    .unwrap_or(false)
            })
    }

    fn file_for(&self, partition: &str, id: &NoteId) -> Option<PathBuf> {
        self.file_for_in(&self.partition_root(partition).join("notes"), id)
    }

    // ------------------------------------------------------------------------
    // Placeholders
    // ------------------------------------------------------------------------

    fn id_of(&self, v: &str) -> NoteId {
        if let Some(label) = v.strip_prefix("@id:") {
            return *self
                .labels
                .get(label)
                .unwrap_or_else(|| panic!("unknown label {label}"));
        }
        NoteId::parse(v).unwrap_or_else(|e| panic!("{e}"))
    }

    fn revision_of_placeholder(&self, v: &str) -> String {
        if v == "@stale" {
            return "f".repeat(64);
        }
        if let Some(rest) = v.strip_prefix("@rev:") {
            let id = self.id_of(rest.strip_prefix("@id:").map(|_| rest).unwrap_or(rest));
            let id = if rest.starts_with("@id:") {
                self.id_of(rest)
            } else {
                id
            };
            return self
                .locate(&id)
                .map(|(_, v)| v.revision)
                .unwrap_or_else(|| panic!("no current revision for {id}"));
        }
        if let Some(label) = v.strip_prefix("@from_rev:") {
            return self.pre_revisions[label].clone();
        }
        assert!(is_revision(v), "revision {v}");
        v.to_string()
    }

    fn expected_revision(&self, v: &str, partition: &str, id: &NoteId) -> String {
        match v {
            "@current" => {
                let path = self
                    .file_for(partition, id)
                    .unwrap_or_else(|| panic!("@current: no file for {id} in {partition}"));
                revision_of(&std::fs::read(path).unwrap())
            }
            "@from_rev" => self
                .last_crash
                .as_ref()
                .and_then(|(from, _)| from.clone())
                .expect("@from_rev after a crash_at"),
            "@to_rev" => self
                .last_crash
                .as_ref()
                .expect("@to_rev after a crash_at")
                .1
                .clone(),
            literal => literal.to_string(),
        }
    }

    // ------------------------------------------------------------------------
    // Running
    // ------------------------------------------------------------------------

    fn run(&mut self, fixture: &Value) {
        for op in arr(&fixture["operations"]) {
            let step = op["step"].to_string();
            let name = s(&op["op"]);
            if SKIPPED_OPS.contains(&name) {
                continue;
            }
            self.warnings.clear();
            let before = self.pre_hashes(&op["expect"]);
            let outcome = self.run_op(op);
            if let Some(reason) = &outcome.skipped {
                eprintln!("step {step} ({name}): skipped — {reason}");
                continue;
            }
            self.check(
                &op["expect"],
                &outcome,
                &before,
                &format!("step {step} ({name})"),
            );
        }
        if let Some(final_state) = fixture.get("final_state") {
            let outcome = Outcome::default();
            let before = self.pre_hashes(final_state);
            self.check(final_state, &outcome, &before, "final_state");
        }
    }

    fn run_op(&mut self, op: &Value) -> Outcome {
        let name = s(&op["op"]);
        let input = &op["input"];
        let ctx = op["context"].as_str().map(|c| {
            self.contexts
                .get(c)
                .cloned()
                .unwrap_or_else(|| panic!("context {c}"))
        });
        match name {
            "search" | "recall" => {
                let ctx = ctx.expect("context");
                self.read_partitions_of(&ctx);
                Outcome {
                    index_only: true,
                    ..Default::default()
                }
            }
            "store" => {
                let ctx = ctx.expect("context");
                self.run_store(&ctx, input, op["as"].as_str())
            }
            "update" => {
                let ctx = ctx.expect("context");
                self.run_update(&ctx, input, op["as"].as_str())
            }
            "forget" => {
                let ctx = ctx.expect("context");
                if op["expect"]["refused"].as_str() == Some("fixture:confirmation_unavailable") {
                    return Outcome::skipped("the unattended refusal is the sandbox's (slice 3)");
                }
                self.run_forget(&ctx, input)
            }
            "read" => {
                let ctx = ctx.expect("context");
                let id = self.id_of(s(&input["id"]));
                let partitions = self.read_partitions_of(&ctx);
                let found = partitions
                    .iter()
                    .find_map(|p| self.partition(p).read(&id).ok().map(|v| (p.clone(), v)));
                match found {
                    Some((partition, view)) => Outcome {
                        ids: Some(vec![view.note.id]),
                        views: vec![(partition, view)],
                        ..Default::default()
                    },
                    None => Outcome {
                        error: Some(MemoryFilesError::NotFound(id)),
                        ..Default::default()
                    },
                }
            }
            "list" => {
                let ctx = ctx.expect("context");
                self.run_list(&ctx, input)
            }
            // Design 09 §3.3: undo and a dismissed suggestion — a forget
            // whose tombstone keeps no link and suppresses no source.
            "withdraw" => {
                let ctx = ctx.expect("context");
                let id = self.id_of(s(&input["id"]));
                let rev = self.revision_of_placeholder(s(&input["expected_revision"]));
                let partitions = self.read_partitions_of(&ctx);
                let partition = self
                    .locate(&id)
                    .map(|(p, _)| p)
                    .filter(|p| partitions.contains(p))
                    .unwrap_or_else(|| partitions[0].clone());
                let outcome = self
                    .partition(&partition)
                    .forget_as(&[(id, rev)], ForgetMode::Withdraw)
                    .unwrap();
                Outcome {
                    forget: Some(BTreeMap::from([(id, outcome[0].status.clone())])),
                    ..Default::default()
                }
            }
            "scan" => {
                for p in self.opened.values() {
                    if p.health() == PartitionHealth::Ready {
                        p.reconcile().unwrap();
                    }
                }
                Outcome::default()
            }
            "external_edit" => {
                let partition = s(&input["partition"]);
                let id = self.id_of(s(&input["id"]));
                let path = self.file_for(partition, &id).expect("file to edit");
                let mut bytes = std::fs::read(&path).unwrap();
                if let Some(text) = input["append"].as_str() {
                    bytes.extend_from_slice(text.as_bytes());
                }
                if let Some(body) = input["replace_body"].as_str() {
                    let mut note = Note::parse(&bytes).unwrap();
                    note.body = body.to_string();
                    bytes = note.serialize().into_bytes();
                }
                if let Some(fm) = input["frontmatter"].as_object() {
                    let mut text = String::from_utf8(bytes).unwrap();
                    for (k, v) in fm {
                        let line = format!("{k}: {}\n", v.as_str().unwrap_or(&v.to_string()));
                        text = text.replacen("---\n", &format!("---\n{line}"), 1);
                    }
                    bytes = text.into_bytes();
                }
                std::fs::write(&path, bytes).unwrap();
                Outcome::default()
            }
            "external_delete" => {
                let partition = s(&input["partition"]);
                let id = self.id_of(s(&input["id"]));
                let path = self.file_for(partition, &id).expect("file to delete");
                std::fs::remove_file(path).unwrap();
                Outcome::default()
            }
            "external_create" => {
                let partition = s(&input["partition"]);
                let (_, bytes, file_name, _) = note_bytes(&input["note"]);
                let notes_dir = self.partition_root(partition).join("notes");
                std::fs::write(notes_dir.join(file_name), bytes).unwrap();
                Outcome::default()
            }
            "restart" => {
                self.open_all();
                Outcome::default()
            }
            "crash_at" => {
                let ctx = ctx.expect("context");
                self.run_crash_at(&ctx, input, op["as"].as_str())
            }
            "concurrent" => {
                let ops = arr(&input["ops"]);
                // Every op here is a store; run them at once, on threads,
                // against the same open partitions, and judge them after
                // the scope ends.
                let mut prepared = Vec::new();
                for inner in ops {
                    let c = self.contexts[s(&inner["context"])].clone();
                    let kind = NoteKind::parse(s(&inner["input"]["kind"])).unwrap();
                    let partition =
                        self.write_partition_of(&c, kind, inner["input"]["scope"].as_str());
                    let new_note = self.new_note_from(&inner["input"]);
                    prepared.push((partition, new_note, c.actor, inner.clone()));
                }
                let results: Vec<(Value, Result<Created, MemoryFilesError>)> =
                    std::thread::scope(|scope| {
                        let handles: Vec<_> = prepared
                            .into_iter()
                            .map(|(partition, new_note, actor, inner)| {
                                let p: &Partition = self.partition(&partition);
                                (inner, scope.spawn(move || p.create(new_note, actor)))
                            })
                            .collect();
                        handles
                            .into_iter()
                            .map(|(inner, handle)| (inner, handle.join().unwrap()))
                            .collect()
                    });
                for (inner, result) in results {
                    let outcome = match result {
                        Ok(c) => {
                            if let Some(label) = inner["as"].as_str() {
                                self.labels.insert(label.to_string(), c.id);
                            }
                            Outcome {
                                created: Some(c),
                                ..Default::default()
                            }
                        }
                        Err(e) => Outcome {
                            error: Some(e),
                            ..Default::default()
                        },
                    };
                    let before = self.pre_hashes(&inner["expect"]);
                    self.check(&inner["expect"], &outcome, &before, "concurrent op");
                }
                Outcome::default()
            }
            "ensure_project_store" => {
                let ws = self.resolve_path(s(&input["workspace"]));
                content_dir(&StoreScope::Project(ws), ContentKind::Artifacts).unwrap();
                Outcome::default()
            }
            "doctor" => Outcome {
                doctor: Some(self.doctor()),
                ..Default::default()
            },
            "doctor_repair" => {
                let current = owner_key(&self.local_user_id);
                let mut error = None;
                for (name, p) in &self.opened {
                    if let Err(e) = p.repair(&current) {
                        eprintln!("repair {name}: {e}");
                        error = Some(e);
                    }
                }
                Outcome {
                    error,
                    ..Default::default()
                }
            }
            "remove_partition" => {
                let partition = s(&input["partition"]);
                let root = self.partition_root(partition);
                let key = self.partition(partition).partition_key().to_string();
                self.opened.remove(partition);
                let _ = std::fs::remove_dir_all(root);
                let _ = std::fs::remove_file(snapshot_path_for(&key).unwrap());
                Outcome::default()
            }
            "adopt" => {
                let now = (self.clock)();
                match adopt(s(&input["owner_key"]), &self.local_user_id, now) {
                    Ok(list) => {
                        self.open_all();
                        Outcome {
                            adopted: Some(list.len()),
                            ..Default::default()
                        }
                    }
                    Err(e) => Outcome {
                        error: Some(e),
                        ..Default::default()
                    },
                }
            }
            other => panic!("unsupported op {other}"),
        }
    }

    fn new_note_from(&self, input: &Value) -> NewNote {
        let title = s(&input["title"]).to_string();
        let kind = input["kind"].as_str().and_then(NoteKind::parse);
        let mut new_note = NewNote {
            title: title.clone(),
            kind,
            aliases: input["aliases"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| s(a).to_string())
                .collect(),
            tags: input["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| s(a).to_string())
                .collect(),
            fact_key: input["fact_key"].as_str().map(str::to_string),
            sources: input["sources"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|src| NoteSource {
                    kind: SourceType::parse(s(&src["type"])).unwrap(),
                    id: s(&src["id"]).to_string(),
                    observed_at: src["observed_at"].as_str().map(parse_time),
                })
                .collect(),
            valid_from: input["valid_from"].as_str().map(parse_time),
            valid_to: input["valid_to"].as_str().map(parse_time),
            // Learning's own inputs (design 09 §2.5): the status the
            // extractor asks for and how a `supersedes` is applied (S2).
            status: input["status"]
                .as_str()
                .map(|v| NoteStatus::parse(v).unwrap_or_else(|| panic!("status {v}"))),
            replace_policy: match input["replace_policy"].as_str() {
                None | Some("direct") => ReplacePolicy::Direct,
                Some("suggest_unless_learned") => ReplacePolicy::SuggestUnlessLearned,
                Some(other) => panic!("replace_policy {other}"),
            },
            supersedes: input["supersedes"].as_object().map(|sup| {
                let id = self.id_of(s(&sup["id"]));
                (
                    id,
                    self.revision_of_placeholder(s(&sup["expected_revision"])),
                )
            }),
            ..Default::default()
        };
        new_note.body = match &input["body"] {
            Value::String(b) => b.clone(),
            Value::Object(o) => {
                let generator = &o["@repeat"];
                let text = s(&generator["text"]);
                let bytes = generator["bytes"].as_u64().unwrap() as usize;
                // Measure the canon's frontmatter for this note with an empty
                // body (every field is fixed-width), then size the body so the
                // whole file is `bytes + 600` (see FIXTURE_FRONTMATTER_BUDGET).
                let probe = Note {
                    id: NoteId::new(),
                    title: title.clone(),
                    kind: kind.unwrap_or(NoteKind::Reference),
                    status: NoteStatus::Active,
                    fact_key: new_note.fact_key.clone(),
                    aliases: new_note.aliases.clone(),
                    tags: new_note.tags.clone(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    observed_at: None,
                    valid_from: None,
                    valid_to: None,
                    review_after: None,
                    pinned: false,
                    sources: new_note.sources.clone(),
                    supersedes: Vec::new(),
                    unknown: Vec::new(),
                    body: String::new(),
                };
                let frontmatter = probe.serialize().len();
                let target = bytes + FIXTURE_FRONTMATTER_BUDGET - frontmatter;
                let mut body = String::new();
                while body.len() < target {
                    body.push_str(text);
                }
                while body.len() > target {
                    body.pop();
                }
                body
            }
            other => panic!("body {other}"),
        };
        new_note
    }

    fn run_store(&mut self, ctx: &Context, input: &Value, label: Option<&str>) -> Outcome {
        let kind = match input["kind"].as_str().and_then(NoteKind::parse) {
            Some(k) => k,
            None => {
                return Outcome {
                    error: Some(MemoryFilesError::InvalidInput("kind".into())),
                    ..Default::default()
                };
            }
        };
        let partition = self.write_partition_of(ctx, kind, input["scope"].as_str());
        let new_note = self.new_note_from(input);
        let actor = ctx.actor;
        let result = self.partition(&partition).create(new_note, actor);
        match result {
            Ok(created) => {
                if let Some(label) = label {
                    self.labels.insert(label.to_string(), created.id);
                }
                let scope = match self.partitions[&partition].scope {
                    StoreScope::Home => "global",
                    StoreScope::Project(_) => "project",
                };
                Outcome {
                    ids: Some(vec![created.id]),
                    created: Some(created),
                    scope: Some(scope.to_string()),
                    ..Default::default()
                }
            }
            Err(e) => Outcome {
                error: Some(e),
                ..Default::default()
            },
        }
    }

    fn update_action(&self, input: &Value) -> UpdateAction {
        match s(&input["action"]) {
            "archive" => UpdateAction::Archive,
            // S5 (design 09 §3.2): a suggestion's confirm names its target's
            // revision.
            "confirm" => UpdateAction::Confirm {
                target_revision: input["target_revision"]
                    .as_str()
                    .map(|v| self.revision_of_placeholder(v)),
            },
            "edit" => {
                let patch = &input["patch"];
                let list = |v: &Value| {
                    v.as_array()
                        .map(|a| a.iter().map(|x| s(x).to_string()).collect())
                };
                UpdateAction::Edit(NotePatch {
                    title: patch["title"].as_str().map(str::to_string),
                    body: patch["body"].as_str().map(str::to_string),
                    aliases: list(&patch["aliases"]),
                    tags: list(&patch["tags"]),
                    fact_key: patch
                        .get("fact_key")
                        .map(|v| v.as_str().map(str::to_string)),
                    valid_from: patch.get("valid_from").map(|v| v.as_str().map(parse_time)),
                    valid_to: patch.get("valid_to").map(|v| v.as_str().map(parse_time)),
                    review_after: patch
                        .get("review_after")
                        .map(|v| v.as_str().map(parse_time)),
                })
            }
            other => panic!("action {other}"),
        }
    }

    fn run_update(&mut self, ctx: &Context, input: &Value, label: Option<&str>) -> Outcome {
        let id = self.id_of(s(&input["id"]));
        let expected = self.revision_of_placeholder(s(&input["expected_revision"]));
        let action = self.update_action(input);
        let partitions = self.read_partitions_of(ctx);
        let Some(partition) = partitions
            .iter()
            .find(|p| self.partition(p).read(&id).is_ok())
            .cloned()
        else {
            return Outcome {
                error: Some(MemoryFilesError::NotFound(id)),
                ..Default::default()
            };
        };
        if let Some(label) = label {
            let current = self.partition(&partition).read(&id).unwrap().revision;
            self.pre_revisions.insert(label.to_string(), current);
        }
        match self
            .partition(&partition)
            .update(&id, &expected, action, ctx.actor)
        {
            Ok(u) => Outcome {
                updated: Some(u),
                ..Default::default()
            },
            Err(e) => Outcome {
                error: Some(e),
                ..Default::default()
            },
        }
    }

    fn run_forget(&mut self, ctx: &Context, input: &Value) -> Outcome {
        let targets: Vec<(NoteId, String)> = arr(&input["targets"])
            .iter()
            .map(|t| {
                let id = self.id_of(s(&t["id"]));
                let rev = s(&t["expected_revision"]);
                // `@rev:<id>` of a note that no longer exists (P12: a forget of
                // a gone id is `not_found` whatever revision is sent).
                let resolvable = rev == "@stale"
                    || rev.starts_with("@from_rev:")
                    || is_revision(rev)
                    || self.locate(&id).is_some();
                let rev = if resolvable {
                    self.revision_of_placeholder(rev)
                } else {
                    "f".repeat(64)
                };
                (id, rev)
            })
            .collect();
        let partitions = self.read_partitions_of(ctx);
        let mut statuses = BTreeMap::new();
        for (id, rev) in targets {
            let home = self.locate(&id).map(|(p, _)| p);
            let partition = home
                .filter(|p| partitions.contains(p))
                .unwrap_or_else(|| partitions[0].clone());
            let outcome = self.partition(&partition).forget(&[(id, rev)]).unwrap();
            statuses.insert(id, outcome[0].status.clone());
        }
        Outcome {
            forget: Some(statuses),
            ..Default::default()
        }
    }

    fn run_list(&mut self, ctx: &Context, input: &Value) -> Outcome {
        let partitions = self.read_partitions_of(ctx);
        let include_history = input["include_history"].as_bool().unwrap_or(false);
        let include_candidates = input["include_candidates"].as_bool().unwrap_or(false);
        let status = input["status"].as_str();
        let kind = input["kind"].as_str().and_then(NoteKind::parse);
        let scope = input["scope"].as_str();
        let mut views = Vec::new();
        for name in partitions {
            let is_project = matches!(self.partitions[&name].scope, StoreScope::Project(_));
            match scope {
                Some("project") if !is_project => continue,
                Some("global") if is_project => continue,
                _ => {}
            }
            for v in self.partition(&name).load().unwrap().views {
                if let Some(k) = kind
                    && v.note.kind != k
                {
                    continue;
                }
                let eff = v.derived.effective_status;
                let keep = match status {
                    Some("archived") => v.note.status == NoteStatus::Archived,
                    Some("candidate") => eff == EffectiveStatus::Candidate,
                    Some("active") => eff == EffectiveStatus::Active,
                    _ => {
                        eff == EffectiveStatus::Active
                            || (include_history
                                && matches!(
                                    eff,
                                    EffectiveStatus::Superseded | EffectiveStatus::Archived
                                ))
                            || (include_candidates && eff == EffectiveStatus::Candidate)
                    }
                };
                if keep {
                    views.push((name.clone(), v));
                }
            }
        }
        let limit = input["limit"].as_u64().unwrap_or(50) as usize;
        views.sort_by(|a, b| {
            b.1.note
                .updated_at
                .cmp(&a.1.note.updated_at)
                .then(a.1.note.id.cmp(&b.1.note.id))
        });
        views.truncate(limit);
        Outcome {
            ids: Some(views.iter().map(|(_, v)| v.note.id).collect()),
            views,
            ..Default::default()
        }
    }

    fn run_crash_at(&mut self, ctx: &Context, input: &Value, label: Option<&str>) -> Outcome {
        let mutation = s(&input["mutation"]);
        let after = input["after_step"].to_string().replace('"', "");
        let point = match (mutation, after.as_str()) {
            ("forget", "1") => CrashPoint::ForgetAfter1,
            ("forget", "2") => CrashPoint::ForgetAfter2,
            ("forget", "3") => return Outcome::skipped("forget step 3 is the index purge"),
            (_, "1") => CrashPoint::WriteAfter1,
            (_, "2a") => CrashPoint::WriteAfter2a,
            (_, "2") => CrashPoint::WriteAfter2,
            (_, "3") => CrashPoint::WriteAfter3,
            other => panic!("crash_at {other:?}"),
        };
        let inner = &input["input"];
        let (partition, id, from_rev) = match mutation {
            "create" => {
                let kind = NoteKind::parse(s(&inner["kind"])).unwrap();
                (
                    self.write_partition_of(ctx, kind, inner["scope"].as_str()),
                    None,
                    None,
                )
            }
            "forget" => {
                let id = self.id_of(s(&inner["targets"][0]["id"]));
                let (p, v) = self.locate(&id).expect("forget target");
                (p, Some(id), Some(v.revision))
            }
            _ => {
                let id = self.id_of(s(&inner["id"]));
                let (p, v) = self.locate(&id).expect("update target");
                (p, Some(id), Some(v.revision))
            }
        };
        let snapshot_before = self.partition(&partition).snapshot().unwrap();
        self.partition(&partition).set_crash_point(Some(point));
        let result: Result<(), MemoryFilesError> = match mutation {
            "create" => {
                let new_note = self.new_note_from(inner);
                self.partition(&partition)
                    .create(new_note, ctx.actor)
                    .map(|_| ())
            }
            "forget" => {
                let rev =
                    self.revision_of_placeholder(s(&inner["targets"][0]["expected_revision"]));
                self.partition(&partition)
                    .forget(&[(id.unwrap(), rev)])
                    .map(|_| ())
            }
            _ => {
                let rev = self.revision_of_placeholder(s(&inner["expected_revision"]));
                self.partition(&partition)
                    .update(&id.unwrap(), &rev, self.update_action(inner), ctx.actor)
                    .map(|_| ())
            }
        };
        match result {
            Err(MemoryFilesError::Crashed(p)) => assert_eq!(p, point),
            other => panic!("expected a crash after {point:?}, got {other:?}"),
        }
        // What the interrupted mutation was writing, read back from disk.
        let on_disk = ControlSnapshot::load(self.partition(&partition).snapshot_path())
            .unwrap()
            .unwrap();
        let (crashed_id, to_rev) = if let Some(p) = &on_disk.pending {
            (p.id, p.to_rev.clone())
        } else if mutation == "forget" {
            (id.unwrap(), String::new())
        } else {
            // After step 3: the record changed.
            let (cid, rec) = on_disk
                .notes
                .iter()
                .find(|(cid, rec)| {
                    snapshot_before.notes.get(cid).map(|r| &r.revision) != Some(&rec.revision)
                })
                .expect("a record changed at step 3");
            (*cid, rec.revision.clone())
        };
        if let Some(label) = label {
            self.labels.insert(label.to_string(), crashed_id);
        }
        self.last_crash = Some((from_rev, to_rev));
        if let Some(text) = input["then_external_edit"].as_str() {
            let path = self
                .file_for(&partition, &crashed_id)
                .expect("file to edit after the crash");
            let mut bytes = std::fs::read(&path).unwrap();
            bytes.extend_from_slice(text.as_bytes());
            std::fs::write(&path, bytes).unwrap();
        }
        Outcome::default()
    }

    fn doctor(&self) -> DoctorReport {
        let mut report = DoctorReport::default();
        let current = owner_key(&self.local_user_id);
        for (name, p) in &self.opened {
            let loaded = p.load().unwrap();
            if matches!(loaded.health, PartitionHealth::Quarantined(_)) {
                report.quarantined.push(name.clone());
            }
            if p.owner_key() != current {
                report.orphans.push(p.owner_key().to_string());
            }
            for c in loaded.conflicts {
                report
                    .conflicts
                    .push((c.kind.as_str().to_string(), c.note, c.target));
            }
            for m in loaded.malformed {
                report.malformed.push((name.clone(), m.path));
            }
            for (id, files) in loaded.duplicates {
                report.duplicate_ids.push(id);
                for f in files {
                    report.malformed.push((name.clone(), f));
                }
            }
        }
        report
    }

    // ------------------------------------------------------------------------
    // Expectations
    // ------------------------------------------------------------------------

    fn pre_hashes(&self, expect: &Value) -> HashMap<String, Option<String>> {
        let mut out = HashMap::new();
        for path in expect["file_unchanged"].as_array().into_iter().flatten() {
            let p = self.resolve_path(s(path));
            out.insert(
                s(path).to_string(),
                std::fs::read(&p).ok().map(|b| sha256_hex(&b)),
            );
        }
        out
    }

    fn check(
        &mut self,
        expect: &Value,
        outcome: &Outcome,
        before: &HashMap<String, Option<String>>,
        at: &str,
    ) {
        let Some(map) = expect.as_object() else {
            return;
        };
        let is_index_op = outcome.index_only;
        for (key, value) in map {
            match key.as_str() {
                k if OTHER_LAYER_KEYS.contains(&k) => {}
                "ids"
                | "ids_unordered"
                | "ids_include"
                | "ids_absent"
                | "count"
                | "results"
                | "refused"
                | "message_contains"
                | "message_not_contains"
                    if is_index_op => {}
                "warnings" => {
                    let count = value["count"].as_u64().unwrap() as usize;
                    assert_eq!(
                        self.warnings.len(),
                        count,
                        "{at}: warnings {:?}",
                        self.warnings
                    );
                    for needle in value["contains"].as_array().into_iter().flatten() {
                        assert!(
                            self.warnings.iter().any(|w| w.contains(s(needle))),
                            "{at}: no warning contains {needle}: {:?}",
                            self.warnings
                        );
                    }
                }
                "refused" => {
                    let e = outcome
                        .error
                        .as_ref()
                        .unwrap_or_else(|| panic!("{at}: expected refusal {value}, got success"));
                    assert_eq!(e.class(), s(value), "{at}: refusal class ({e})");
                }
                "message_contains" => {
                    let e = outcome
                        .error
                        .as_ref()
                        .unwrap_or_else(|| panic!("{at}: no error"));
                    let msg = e.to_string();
                    for needle in arr(value) {
                        assert!(
                            msg.contains(s(needle)),
                            "{at}: message `{msg}` lacks {needle}"
                        );
                    }
                }
                "message_not_contains" => {
                    let msg = outcome
                        .error
                        .as_ref()
                        .map(|e| e.to_string())
                        .unwrap_or_default();
                    for needle in arr(value) {
                        assert!(
                            !msg.contains(s(needle)),
                            "{at}: message `{msg}` contains {needle}"
                        );
                    }
                }
                "created" => {
                    let want = value.as_bool().unwrap();
                    assert_eq!(
                        outcome.created.is_some(),
                        want,
                        "{at}: created ({:?})",
                        outcome.error
                    );
                }
                "dedup" => {
                    let c = outcome.created.as_ref().unwrap();
                    let want = match s(value) {
                        "created" => Dedup::Created,
                        _ => Dedup::Existing,
                    };
                    assert_eq!(c.dedup, want, "{at}: dedup");
                }
                "scope" => {
                    if let Some(scope) = &outcome.scope {
                        assert_eq!(scope, s(value), "{at}: scope");
                    }
                }
                "origin" => {
                    let c = outcome
                        .created
                        .as_ref()
                        .unwrap_or_else(|| panic!("{at}: origin without a create"));
                    assert_eq!(c.origin.as_str(), s(value), "{at}: origin");
                }
                "superseded" => {
                    let c = outcome.created.as_ref().unwrap();
                    let want: Vec<NoteId> = arr(value).iter().map(|v| self.id_of(s(v))).collect();
                    assert_eq!(c.superseded, want, "{at}: superseded");
                }
                "suggestion" => {
                    let c = outcome
                        .created
                        .as_ref()
                        .unwrap_or_else(|| panic!("{at}: suggestion without a create"));
                    assert_eq!(c.suggestion, value.as_bool().unwrap(), "{at}: suggestion");
                }
                "effective_status" => {
                    for (id, want) in obj(value) {
                        let id = self.id_of(id);
                        assert_eq!(
                            self.locate(&id)
                                .unwrap()
                                .1
                                .derived
                                .effective_status
                                .as_str(),
                            s(want),
                            "{at}: effective_status {id}"
                        );
                    }
                }
                "revision_changed" => {
                    let want = value.as_bool().unwrap();
                    let changed = outcome.updated.as_ref().map(|u| u.written).unwrap_or(false);
                    assert_eq!(changed, want, "{at}: revision_changed");
                }
                "current_revision" => {
                    let id = self.id_of(s(value));
                    let current = self.locate(&id).unwrap().1.revision;
                    match outcome.error.as_ref() {
                        Some(MemoryFilesError::RevisionConflict {
                            current_revision, ..
                        }) => {
                            assert_eq!(current_revision, &current, "{at}: current_revision");
                        }
                        other => panic!("{at}: expected a revision conflict, got {other:?}"),
                    }
                }
                "status" => {
                    if let Some(u) = &outcome.updated {
                        assert_eq!(u.status.as_str(), s(value), "{at}: status");
                    } else if let Some(f) = &outcome.forget {
                        for (id, want) in obj(value) {
                            let id = self.id_of(id);
                            let got = match &f[&id] {
                                ForgetStatus::Forgotten => "forgotten",
                                ForgetStatus::RevisionConflict { .. } => "revision_conflict",
                                ForgetStatus::NotFound => "not_found",
                            };
                            assert_eq!(got, s(want), "{at}: forget status of {id}");
                        }
                    } else if let Some((_, v)) = outcome.views.first() {
                        assert_eq!(
                            v.derived.effective_status.as_str(),
                            s(value),
                            "{at}: status"
                        );
                    } else if outcome.error.is_some() {
                        // A refused update: the note's status is unchanged.
                        let id = outcome.error.as_ref().and_then(|e| match e {
                            MemoryFilesError::AuthorityConflict { id, .. }
                            | MemoryFilesError::ConfirmRefused { id, .. } => Some(*id),
                            _ => None,
                        });
                        if let Some(id) = id {
                            assert_eq!(
                                self.locate(&id).unwrap().1.note.status.as_str(),
                                s(value),
                                "{at}: status"
                            );
                        }
                    } else {
                        panic!("{at}: status on an outcome without one");
                    }
                }
                "note_status" => {
                    for (id, want) in obj(value) {
                        let id = self.id_of(id);
                        assert_eq!(
                            self.locate(&id).unwrap().1.note.status.as_str(),
                            s(want),
                            "{at}: note_status {id}"
                        );
                    }
                }
                "ids" => {
                    let got = outcome
                        .ids
                        .clone()
                        .unwrap_or_else(|| panic!("{at}: ids on an op without ids"));
                    let want: Vec<NoteId> = arr(value).iter().map(|v| self.id_of(s(v))).collect();
                    assert_eq!(got, want, "{at}: ids");
                }
                "ids_unordered" => {
                    let mut got = outcome
                        .ids
                        .clone()
                        .unwrap_or_else(|| panic!("{at}: ids on an op without ids"));
                    let mut want: Vec<NoteId> =
                        arr(value).iter().map(|v| self.id_of(s(v))).collect();
                    got.sort();
                    want.sort();
                    assert_eq!(got, want, "{at}: ids_unordered");
                }
                "ids_include" => {
                    let got = outcome.ids.clone().unwrap_or_default();
                    for v in arr(value) {
                        assert!(got.contains(&self.id_of(s(v))), "{at}: ids_include {v}");
                    }
                }
                "ids_absent" => {
                    let got = outcome.ids.clone().unwrap_or_default();
                    for v in arr(value) {
                        assert!(!got.contains(&self.id_of(s(v))), "{at}: ids_absent {v}");
                    }
                }
                "count" => {
                    let got = outcome.ids.as_ref().map(Vec::len).unwrap_or(0);
                    assert_eq!(got, value.as_u64().unwrap() as usize, "{at}: count");
                }
                "results" => {
                    for (id, fields) in obj(value) {
                        let id = self.id_of(id);
                        let Some((partition, v)) =
                            outcome.views.iter().find(|(_, v)| v.note.id == id)
                        else {
                            panic!("{at}: results for {id} which the op did not return");
                        };
                        for (f, want) in obj(fields) {
                            match f.as_str() {
                                "status" => assert_eq!(
                                    v.derived.effective_status.as_str(),
                                    s(want),
                                    "{at}: {id} status"
                                ),
                                "origin" => assert_eq!(
                                    v.derived.origin.as_str(),
                                    s(want),
                                    "{at}: {id} origin"
                                ),
                                "scope" => {
                                    let scope = match self.partitions[partition].scope {
                                        StoreScope::Home => "global",
                                        StoreScope::Project(_) => "project",
                                    };
                                    assert_eq!(scope, s(want), "{at}: {id} scope");
                                }
                                "conflict" => assert_eq!(
                                    v.derived.conflict,
                                    want.as_bool().unwrap(),
                                    "{at}: {id} conflict"
                                ),
                                "superseded_by" => {
                                    let want: Vec<NoteId> =
                                        arr(want).iter().map(|x| self.id_of(s(x))).collect();
                                    assert_eq!(
                                        v.derived.superseded_by, want,
                                        "{at}: {id} superseded_by"
                                    );
                                }
                                "excerpt_contains" => assert!(
                                    v.note.body.contains(s(want)),
                                    "{at}: {id} body lacks {want}"
                                ),
                                "sources_status" => {
                                    let want_forgotten = s(want) == "source_forgotten";
                                    assert_eq!(
                                        v.derived.source_forgotten, want_forgotten,
                                        "{at}: {id} sources_status"
                                    );
                                }
                                "freshness" | "match_includes" | "valid_from" | "valid_to" => {}
                                other => panic!("{at}: unknown result field {other}"),
                            }
                        }
                    }
                }
                "truncated" => {
                    if let Some((_, v)) = outcome.views.first() {
                        assert!(
                            !value.as_bool().unwrap() && v.note.body.len() <= MAX_NOTE_BYTES,
                            "{at}: truncated"
                        );
                    }
                }
                "body_bytes_lte" => {
                    let (_, v) = outcome.views.first().unwrap();
                    assert!(
                        v.note.body.len() <= value.as_u64().unwrap() as usize,
                        "{at}: body_bytes_lte"
                    );
                }
                "frontmatter_aliases" => {
                    let (_, v) = outcome.views.first().unwrap();
                    let want: Vec<&str> = arr(value).iter().map(s).collect();
                    assert_eq!(v.note.aliases, want, "{at}: aliases");
                }
                "snapshot" => {
                    for (partition, spec) in obj(value) {
                        self.check_snapshot(partition, spec, at);
                    }
                }
                "snapshot_missing" => {
                    for partition in arr(value) {
                        let name = s(partition);
                        let spec = &self.partitions[name];
                        let control = self.home.join("state").join("memory-control");
                        let prefix = match spec.scope {
                            StoreScope::Home => format!("home-{}", owner_key(&spec.owner)),
                            StoreScope::Project(_) => format!("proj-{}-", owner_key(&spec.owner)),
                        };
                        let any = std::fs::read_dir(&control)
                            .ok()
                            .into_iter()
                            .flatten()
                            .flatten()
                            .any(|e| e.file_name().to_string_lossy().starts_with(&prefix));
                        assert!(!any, "{at}: a snapshot exists for {name}");
                    }
                }
                "files_exist" => {
                    for path in arr(value) {
                        if is_index_file(s(path)) {
                            continue;
                        }
                        let p = self.resolve_path(s(path));
                        assert!(p.exists(), "{at}: {} should exist", p.display());
                    }
                }
                "files_absent" => {
                    for path in arr(value) {
                        if is_index_file(s(path)) {
                            continue;
                        }
                        let p = self.resolve_path(s(path));
                        assert!(
                            std::fs::symlink_metadata(&p).is_err(),
                            "{at}: {} should be absent",
                            p.display()
                        );
                    }
                }
                "file_content" => {
                    for (path, content) in obj(value) {
                        let p = self.resolve_path(path);
                        assert_eq!(
                            std::fs::read(&p).unwrap(),
                            s(content).as_bytes(),
                            "{at}: {}",
                            p.display()
                        );
                    }
                }
                "file_unchanged" => {
                    for path in arr(value) {
                        let p = self.resolve_path(s(path));
                        let now = std::fs::read(&p).ok().map(|b| sha256_hex(&b));
                        assert_eq!(&now, &before[s(path)], "{at}: {} changed", p.display());
                    }
                }
                "notes_on_disk" | "notes_not_on_disk" => {
                    for entry in arr(value) {
                        let id = self.id_of(s(&entry["id"]));
                        let found = self.file_for(s(&entry["partition"]), &id).is_some();
                        assert_eq!(found, key == "notes_on_disk", "{at}: {key} {id}");
                    }
                }
                "history_has" | "history_absent" => {
                    for entry in arr(value) {
                        let partition = s(&entry["partition"]);
                        let id = self.id_of(s(&entry["id"]));
                        let rev = self.expected_revision(s(&entry["revision"]), partition, &id);
                        let path = self
                            .partition_root(partition)
                            .join(".history")
                            .join(id.to_string())
                            .join(format!("{rev}.md"));
                        assert_eq!(
                            path.is_file(),
                            key == "history_has",
                            "{at}: {key} {}",
                            path.display()
                        );
                    }
                }
                "history_count" => {
                    for (partition, ids) in obj(value) {
                        for (id, n) in obj(ids) {
                            let id = self.id_of(id);
                            let dir = self
                                .partition_root(partition)
                                .join(".history")
                                .join(id.to_string());
                            let count = std::fs::read_dir(&dir)
                                .map(|d| d.flatten().count())
                                .unwrap_or(0);
                            assert_eq!(
                                count,
                                n.as_u64().unwrap() as usize,
                                "{at}: history_count {id}"
                            );
                        }
                    }
                }
                "filename_matches" => {
                    for (label, pattern) in obj(value) {
                        let id = self.id_of(&format!("@id:{label}")).to_string();
                        let (partition, view) = self.locate(&NoteId::parse(&id).unwrap()).unwrap();
                        let _ = partition;
                        assert!(
                            simple_regex_match(s(pattern), &view.file_name),
                            "{at}: {} !~ {pattern}",
                            view.file_name
                        );
                    }
                }
                "path_within" => {
                    for (label, dir) in obj(value) {
                        let id = if label.len() == 36 {
                            self.id_of(label)
                        } else {
                            self.id_of(&format!("@id:{label}"))
                        };
                        let (_, view) = self.locate(&id).unwrap();
                        let want = self.resolve_path(s(dir));
                        assert_eq!(
                            view.path.parent().unwrap(),
                            want,
                            "{at}: path_within {label}"
                        );
                    }
                }
                "doctor" => {
                    let report = outcome
                        .doctor
                        .as_ref()
                        .unwrap_or_else(|| panic!("{at}: doctor on a non-doctor op"));
                    for (k, want) in obj(value) {
                        match k.as_str() {
                            "conflicts" => {
                                let mut want: Vec<(String, NoteId, NoteId)> = arr(want)
                                    .iter()
                                    .map(|c| {
                                        (
                                            s(&c["type"]).to_string(),
                                            self.id_of(s(&c["note"])),
                                            self.id_of(s(&c["target"])),
                                        )
                                    })
                                    .collect();
                                let mut got = report.conflicts.clone();
                                want.sort();
                                got.sort();
                                assert_eq!(got, want, "{at}: doctor.conflicts");
                            }
                            "quarantined" => {
                                let mut want: Vec<&str> = arr(want).iter().map(s).collect();
                                let mut got: Vec<&str> =
                                    report.quarantined.iter().map(String::as_str).collect();
                                want.sort();
                                got.sort();
                                assert_eq!(got, want, "{at}: doctor.quarantined");
                            }
                            "orphans" => {
                                let mut want: Vec<&str> = arr(want).iter().map(s).collect();
                                let mut got: Vec<&str> =
                                    report.orphans.iter().map(String::as_str).collect();
                                want.sort();
                                got.sort();
                                assert_eq!(got, want, "{at}: doctor.orphans");
                            }
                            "malformed" => {
                                for entry in arr(want) {
                                    let partition = s(&entry["partition"]);
                                    let needle = s(&entry["file_contains"]);
                                    assert!(
                                        report.malformed.iter().any(|(p, path)| p == partition
                                            && path.to_string_lossy().contains(needle)),
                                        "{at}: doctor.malformed lacks {needle} in {partition}: {:?}",
                                        report.malformed
                                    );
                                }
                            }
                            "duplicate_ids" => {
                                let mut want: Vec<NoteId> = arr(want)
                                    .iter()
                                    .flat_map(|g| arr(g).iter().map(|v| self.id_of(s(v))))
                                    .collect();
                                let mut got = report.duplicate_ids.clone();
                                want.sort();
                                got.sort();
                                assert_eq!(got, want, "{at}: doctor.duplicate_ids");
                            }
                            "stale" | "abandoned" => {}
                            other => panic!("{at}: unknown doctor key {other}"),
                        }
                    }
                }
                "adopted_partitions_count" => {
                    assert_eq!(
                        outcome.adopted,
                        Some(value.as_u64().unwrap() as usize),
                        "{at}: adopted"
                    );
                }
                other => panic!("{at}: unknown expectation key {other}"),
            }
        }
    }

    fn check_snapshot(&self, partition: &str, spec: &Value, at: &str) {
        let name = partition;
        let snapshot_spec = &self.partitions[name];
        let Some(p) = self.opened.get(name) else {
            panic!("{at}: snapshot of {name}, which is not open");
        };
        let on_disk = ControlSnapshot::load(p.snapshot_path())
            .unwrap()
            .unwrap_or_else(|| panic!("{at}: no snapshot for {name}"));
        for (k, want) in obj(spec) {
            match k.as_str() {
                "partition_key" => {
                    if s(want) != "@derived" {
                        assert_eq!(on_disk.partition_key, s(want), "{at}: {name} partition_key");
                    }
                    assert_eq!(on_disk.partition_key, p.partition_key());
                }
                "owner_key" => {
                    assert_eq!(on_disk.owner_key, s(want), "{at}: {name} owner_key");
                    assert_eq!(on_disk.owner_key, owner_key(&snapshot_spec.owner));
                }
                "owner_id" => assert_eq!(on_disk.owner_id, s(want), "{at}: {name} owner_id"),
                "root_is" => {
                    let want = self.resolve_path(s(want));
                    assert_eq!(
                        Path::new(&on_disk.root),
                        want.canonicalize().unwrap_or(want),
                        "{at}: {name} root"
                    );
                }
                "moved_from" => assert_eq!(
                    on_disk.moved_from.as_deref(),
                    want.as_str(),
                    "{at}: {name} moved_from"
                ),
                "pending" => {
                    if want.is_null() {
                        assert!(
                            on_disk.pending.is_none(),
                            "{at}: {name} pending should be null: {:?}",
                            on_disk.pending
                        );
                    } else {
                        let pending = on_disk
                            .pending
                            .as_ref()
                            .unwrap_or_else(|| panic!("{at}: {name} pending expected"));
                        assert_eq!(pending.id, self.id_of(s(&want["id"])), "{at}: pending.id");
                        match &want["from_rev"] {
                            Value::Null => {
                                assert!(pending.from_rev.is_none(), "{at}: pending.from_rev null")
                            }
                            v => assert_eq!(
                                pending.from_rev.as_deref(),
                                Some(self.expected_revision(s(v), name, &pending.id).as_str()),
                                "{at}: pending.from_rev"
                            ),
                        }
                        assert_eq!(
                            pending.to_rev,
                            self.expected_revision(s(&want["to_rev"]), name, &pending.id),
                            "{at}: pending.to_rev"
                        );
                    }
                }
                "notes" => {
                    for (id, rec) in obj(want) {
                        let id = self.id_of(id);
                        if rec.is_null() {
                            assert!(
                                !on_disk.notes.contains_key(&id),
                                "{at}: {name} notes[{id}] should be absent"
                            );
                            continue;
                        }
                        let got = on_disk
                            .notes
                            .get(&id)
                            .unwrap_or_else(|| panic!("{at}: {name} notes[{id}] missing"));
                        if let Some(rev) = rec["revision"].as_str() {
                            assert_eq!(
                                got.revision,
                                self.expected_revision(rev, name, &id),
                                "{at}: {name} notes[{id}].revision"
                            );
                        }
                        if let Some(origin) = rec["origin"].as_str() {
                            assert_eq!(
                                got.origin.as_str(),
                                origin,
                                "{at}: {name} notes[{id}].origin"
                            );
                        }
                    }
                }
                "tombstones" => {
                    for want_t in arr(want) {
                        let id = self.id_of(s(&want_t["id"]));
                        let t = on_disk
                            .tombstone(&id)
                            .unwrap_or_else(|| panic!("{at}: {name} tombstone {id} missing"));
                        if let Some(o) = want_t["origin"].as_str() {
                            assert_eq!(t.origin.as_str(), o, "{at}: tombstone {id} origin");
                        }
                        if let Some(sup) = want_t["supersedes"].as_array() {
                            let want: Vec<NoteId> = sup.iter().map(|v| self.id_of(s(v))).collect();
                            assert_eq!(t.supersedes, want, "{at}: tombstone {id} supersedes");
                        }
                        if let Some(src) = want_t["suppressed_sources"].as_array() {
                            let want: Vec<&str> = src.iter().map(s).collect();
                            assert_eq!(t.suppressed_sources, want, "{at}: tombstone {id} sources");
                        }
                    }
                }
                "tombstones_count" => assert_eq!(
                    on_disk.tombstones.len(),
                    want.as_u64().unwrap() as usize,
                    "{at}: {name} tombstones_count"
                ),
                other => panic!("{at}: unknown snapshot key {other}"),
            }
        }
    }
}

/// `state/cache/memory-index-v1.sqlite` is the index's file (design §2); its
/// presence is the `memory_index` loader's to check.
fn is_index_file(fixture_path: &str) -> bool {
    fixture_path.starts_with("home/state/cache/")
}

fn snapshot_root_notes(snapshot: &ControlSnapshot) -> PathBuf {
    Path::new(&snapshot.root).join("notes")
}

fn tombstone_from(v: &Value) -> Tombstone {
    Tombstone {
        id: NoteId::parse(s(&v["id"])).unwrap(),
        origin: Origin::parse(s(&v["origin"])).unwrap(),
        supersedes: arr(&v["supersedes"])
            .iter()
            .map(|x| NoteId::parse(s(x)).unwrap())
            .collect(),
        suppressed_sources: arr(&v["suppressed_sources"])
            .iter()
            .map(|x| s(x).to_string())
            .collect(),
        forgotten_at: parse_time(s(&v["forgotten_at"])),
    }
}

/// A fixture note → the bytes the loader writes (through the canon, or raw),
/// its file name, and its `updated_at` for the default record.
fn note_bytes(n: &Value) -> (NoteId, Vec<u8>, String, Option<DateTime<Utc>>) {
    let id = NoteId::parse(s(&n["id"]).split("../").next().unwrap_or(""))
        .unwrap_or_else(|_| NoteId::parse("00000000-0000-4000-8000-000000000000").unwrap());
    if let Some(raw) = n["raw"].as_str() {
        return (
            id,
            raw.as_bytes().to_vec(),
            s(&n["filename"]).to_string(),
            None,
        );
    }
    let list = |v: &Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .map(|x| s(x).to_string())
            .collect()
    };
    let note = Note {
        id,
        title: s(&n["title"]).to_string(),
        kind: NoteKind::parse(s(&n["kind"])).unwrap(),
        status: NoteStatus::parse(s(&n["status"])).unwrap(),
        fact_key: n["fact_key"].as_str().map(str::to_string),
        aliases: list(&n["aliases"]),
        tags: list(&n["tags"]),
        created_at: parse_time(s(&n["created_at"])),
        updated_at: parse_time(s(&n["updated_at"])),
        observed_at: n["observed_at"].as_str().map(parse_time),
        valid_from: n["valid_from"].as_str().map(parse_time),
        valid_to: n["valid_to"].as_str().map(parse_time),
        review_after: n["review_after"].as_str().map(parse_time),
        pinned: n["pinned"].as_bool().unwrap_or(false),
        sources: n["sources"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|src| NoteSource {
                kind: SourceType::parse(s(&src["type"])).unwrap(),
                id: s(&src["id"]).to_string(),
                observed_at: src["observed_at"].as_str().map(parse_time),
            })
            .collect(),
        supersedes: list(&n["supersedes"])
            .iter()
            .map(|x| NoteId::parse(x).unwrap())
            .collect(),
        unknown: Vec::new(),
        body: match &n["body"] {
            Value::String(b) => b.clone(),
            Value::Object(o) => {
                let text = s(&o["@repeat"]["text"]);
                let bytes = o["@repeat"]["bytes"].as_u64().unwrap() as usize;
                let mut body = String::new();
                while body.len() < bytes {
                    body.push_str(text);
                }
                while body.len() > bytes {
                    body.pop();
                }
                body
            }
            other => panic!("body {other}"),
        },
    };
    let file_name = note.file_name();
    (
        id,
        note.serialize().into_bytes(),
        file_name,
        Some(note.updated_at),
    )
}

/// The fixtures' file-name patterns are one shape: a UUID, `--`, a literal
/// slug and `\.md`, anchored. Enough of a matcher for that.
fn simple_regex_match(pattern: &str, name: &str) -> bool {
    let inner = pattern.trim_start_matches('^').trim_end_matches('$');
    let uuid = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";
    let rest = inner
        .strip_prefix(uuid)
        .expect("a uuid-prefixed pattern")
        .replace("\\.", ".");
    let Some((id, tail)) = name.split_at_checked(36) else {
        return false;
    };
    NoteId::parse(id).is_ok() && tail == rest
}

#[derive(Default)]
struct DoctorReport {
    quarantined: Vec<String>,
    orphans: Vec<String>,
    conflicts: Vec<(String, NoteId, NoteId)>,
    malformed: Vec<(String, PathBuf)>,
    duplicate_ids: Vec<NoteId>,
}

#[derive(Default)]
struct Outcome {
    error: Option<MemoryFilesError>,
    created: Option<Created>,
    updated: Option<Updated>,
    forget: Option<BTreeMap<NoteId, ForgetStatus>>,
    ids: Option<Vec<NoteId>>,
    views: Vec<(String, NoteView)>,
    scope: Option<String>,
    doctor: Option<DoctorReport>,
    adopted: Option<usize>,
    skipped: Option<String>,
    index_only: bool,
}

impl Outcome {
    fn skipped(reason: &str) -> Self {
        Self {
            skipped: Some(reason.to_string()),
            ..Default::default()
        }
    }
}

fn run_fixture(file: &str) {
    let path = fixtures_dir().join(file);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let fixture: Value = serde_json::from_str(&text).unwrap();
    let loads_in: Vec<&str> = arr(&fixture["loads_in"]).iter().map(s).collect();
    assert!(
        loads_in.contains(&"slice-2"),
        "{file} is not a slice-2 fixture"
    );
    let mut harness = Harness::build(&fixture);
    harness.run(&fixture);
}

#[test]
fn every_file_level_fixture_is_satisfied() {
    for file in FILE_LEVEL_FIXTURES {
        eprintln!("=== {file}");
        run_fixture(file);
    }
}

/// The loader finds a context's project the way the daemon does (storage
/// design §3): a folder holding `.git` is a root, and `.openalpaca` marks
/// nothing any more (ST-9).
#[test]
fn the_loader_resolves_a_workspace_as_the_store_does() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _env = HomeStoreGuard::set(&home);
    let repo = root.join("ws").join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    let old = root.join("ws").join("old");
    std::fs::create_dir_all(old.join(".openalpaca")).unwrap();

    assert_eq!(find_project_root(&repo.join("src")), Some(repo));
    assert_eq!(find_project_root(&old), None, "`.openalpaca` is no marker");
}
