//! `MemoryService` — the one implementation of memory for every agent
//! (design `tasks/memory-redesign/03-design.md` rev 4.2, §3, §5, §6, §7a;
//! plan slice 3 rows 3.1–3.2).
//!
//! It maps the two slice-2 libraries to each other and to the request:
//! `memory_files` owns the notes on disk and the control snapshot;
//! `memory_index` owns the disposable lexical cache; this module owns
//!
//! - the [`MemoryAccessContext`]: the owner (a `global_user` row, else no
//!   owner), the MD-5 project root (the marker walk from the request
//!   workspace, never the daemon's CWD), the surface and the capabilities;
//! - partition resolution with lazy store creation and the per-component
//!   link check (design §3 rule 1), the kind-based default scope (rule 2);
//! - the read path (design §5 steps 1–8): pinned notes, the union-of-terms
//!   search, verification of every selected file, the byte budgets, the
//!   quoted-data delivery and `memory_used`;
//! - store, update (edit/archive/confirm), supersede, forget, undo, read,
//!   list and sync under the origin and authority rules (design §4, §6);
//! - one writer queue per partition and the quiesce hook (design §6, §3);
//! - the in-memory ring `GET /v1/memories/recently-used` reads.
//!
//! Everything here is synchronous file work; an async caller (a tool, a
//! route) runs it on the blocking pool.

mod context;
mod error;
mod in_chat;
mod learning;
mod management;
mod mutations;
pub mod recall;
mod suggestions;
mod types;
pub mod wire;
mod write_slot;

#[cfg(test)]
mod tests;

pub use context::{
    Attribution, Capabilities, MemoryAccessContext, Surface, WorkspaceHint, resolve_project_root,
};
pub use error::MemoryError;
pub use in_chat::{
    PENDING_SUGGESTIONS_READ_MAX, PREVIEW_MAX_BYTES, PendingSuggestion,
    SUGGESTION_LINE_TITLE_BYTES, SuggestionPreview,
};
pub use recall::{TRUST_LABEL, wrap_delivery};
pub use suggestions::{SUGGESTION_EXCERPT_BYTES, SUGGESTIONS_DEFAULT_LIMIT, SUGGESTIONS_MAX_LIMIT};
pub use types::{
    ExtractionProvenance, ForgetInput, ForgetItem, ForgetItemStatus, ForgetOutput, Freshness,
    HistoryEntry, KnownNote, LearningView, ListInput, ListItem, ListOutput, MemoryChange,
    MemoryUsed, Patch, ReadInput, ReadOutput, RecallBudget, RecallResult, RecalledNote,
    RecentlyUsedItem, Retained, SearchInput, SearchOutput, SearchResultItem, StoreInput,
    StoreOutput, Suggestion, SuggestionTarget, UpdateAction, UpdateInput, UpdateOutput, UsedReason,
    WithdrawOutput,
};
pub use write_slot::QuiesceGuard;

use chrono::{DateTime, Utc};
use openalpaca_storage::Database;
use openalpaca_storage::memory_files::{
    self as files, Dedup, EffectiveStatus, LoadedPartition, MAX_LIST_ENTRIES, MAX_NOTE_BYTES,
    MAX_SOURCES, MAX_TITLE_CHARS, MemoryFilesError, NewNote, NoteId, NoteScope, NoteSource,
    NoteStatus, NoteView, Origin, Partition, PartitionHealth, ReplacePolicy, ScanEntry, SourceType,
    is_valid_fact_key,
};
use openalpaca_storage::memory_index::{
    self as index, FileStat, IndexRecord, IndexState, MemoryIndex,
};
use openalpaca_storage::store::StoreScope;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use write_slot::WriteSlots;

/// Design §4 *Initial limits*: the combined automatic memory context.
pub const RECALL_TOTAL_BYTES: usize = 8 * 1024;
/// Design §5 step 2: the pinned sub-budget.
pub const RECALL_PINNED_BYTES: usize = 2 * 1024;
/// Design §4: at most 8 retrieved chunks.
pub const RECALL_MAX_ITEMS: usize = 8;
/// Design §4: the explicit tool read response.
pub const READ_MAX_BYTES: usize = 32 * 1024;
/// `GET /v1/memories/recently-used`: `limit` max 200, the ring's capacity.
pub const RING_CAPACITY: usize = 200;
/// How many requests' deliveries are held for [`MemoryService::take_used`]
/// at once: a request that is never drained is forgotten once this many
/// newer ones have recorded a delivery.
pub const USED_REQUESTS_CAPACITY: usize = 256;
/// `memory_list.limit`: 1–100, default 50.
pub const LIST_DEFAULT_LIMIT: usize = 50;
pub const LIST_MAX_LIMIT: usize = 100;

/// What a workspace move carries from step 2 to step 5 (design §3): the
/// snapshots [`MemoryService::begin_project_move`] re-keyed, for
/// [`MemoryService::finish_project_move`].
#[derive(Debug)]
pub struct ProjectMove {
    moves: Vec<files::SnapshotMove>,
}

impl ProjectMove {
    /// How many partitions (one per owner) the move re-keys.
    pub fn partitions(&self) -> usize {
        self.moves.len()
    }
}

/// One reconciliation's account (`POST /v1/memories/sync`).
#[derive(Debug, Clone)]
pub struct SyncReport {
    pub scanned: usize,
    pub changes: Vec<MemoryChange>,
    pub index_state: IndexState,
}

/// What the index needs to know about a note, so a write re-indexes only
/// what changed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Fingerprint {
    revision: String,
    effective_status: EffectiveStatus,
    origin: Origin,
    conflict: bool,
    same_key_loser: bool,
    source_forgotten: bool,
    superseded_by: Vec<NoteId>,
    pinned: bool,
    relative_path: String,
    stat: (i64, u64, u64),
}

/// The owner's partitions for one request (design §3 rule 1).
pub(super) struct Resolved {
    home: Partition,
    project: Option<Partition>,
}

impl Resolved {
    fn all(&self) -> Vec<&Partition> {
        let mut out = vec![&self.home];
        if let Some(p) = &self.project {
            out.push(p);
        }
        out
    }

    fn in_scope(&self, scope: Option<NoteScope>) -> Vec<&Partition> {
        match scope {
            None => self.all(),
            Some(NoteScope::Global) => vec![&self.home],
            Some(NoteScope::Project) => self.project.iter().collect(),
        }
    }
}

pub struct MemoryService {
    db: Option<Database>,
    index: MemoryIndex,
    slots: WriteSlots,
    fingerprints: Mutex<HashMap<String, HashMap<NoteId, Fingerprint>>>,
    /// The reconciliation timer's last stat scan per partition key, sorted
    /// by file name (design §5: a bounded stat scan, and only a file whose
    /// mtime, size or inode changed is re-hashed). A tick whose scan
    /// matches re-parses and re-indexes nothing.
    last_scans: Mutex<HashMap<String, Vec<ScanEntry>>>,
    ring: Mutex<VecDeque<RecentlyUsedItem>>,
    /// Per request, what it delivered so far in delivery order — the turn's
    /// `memory_used` (design §7a), taken once by the bridge. Oldest first,
    /// bounded by [`USED_REQUESTS_CAPACITY`].
    used_by_request: Mutex<VecDeque<(uuid::Uuid, Vec<MemoryUsed>)>>,
    /// Per lane, the suggestions a `memory_confirm` card was raised for since
    /// the daemon started, and the notes they would replace (design 09 §4.5):
    /// kept out of that lane's `<memory_suggestions>` so a timed-out or
    /// denied card is not raised again in the same conversation, nor another
    /// restatement of the same change. In memory only.
    raised: Mutex<HashMap<String, HashSet<NoteId>>>,
    /// Where `memory_changed` goes (design §7a): every write the service
    /// makes, whichever surface asked, and every change a reconciliation
    /// scan detects. `None` in a test that has no bus; the daemon attaches
    /// its bus with [`MemoryService::with_bus`].
    bus: Option<crate::bus::EventBus>,
}

impl std::fmt::Debug for MemoryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryService")
            .field("db", &self.db.is_some())
            .field("index", &self.index)
            .finish()
    }
}

impl MemoryService {
    /// A service over `index`. `db` is where owners and sources are
    /// verified; without one no owner can be confirmed and every context
    /// construction answers *no owner*.
    ///
    /// Touches nothing on disk: an index that reports `rebuilding` stays so
    /// until [`MemoryService::rebuild_index`] runs — [`Self::open_default`]
    /// does that for the daemon, and a test does it under its own home store.
    pub fn new(db: Option<Database>, index: MemoryIndex) -> Self {
        Self {
            db,
            index,
            slots: WriteSlots::default(),
            fingerprints: Mutex::new(HashMap::new()),
            last_scans: Mutex::new(HashMap::new()),
            ring: Mutex::new(VecDeque::with_capacity(RING_CAPACITY)),
            used_by_request: Mutex::new(VecDeque::new()),
            raised: Mutex::new(HashMap::new()),
            bus: None,
        }
    }

    /// Attaches the bus every `memory_changed` is published on (design
    /// §7a). One publish per write, from inside the service, so a tool call,
    /// the extractor, a route and the reconciliation scan all announce a
    /// change the same way without each remembering to.
    pub fn with_bus(mut self, bus: crate::bus::EventBus) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Publishes one change (design §7a `memory_changed`) when a bus is
    /// attached. The `MemoryChange` stays on the operation's output too, so
    /// a caller can read what it did without subscribing.
    pub(super) fn publish(&self, change: &MemoryChange) {
        if let Some(bus) = &self.bus {
            bus.publish(change.to_event());
        }
    }

    /// The daemon's service: the cache at `state/cache/memory-index-v1.sqlite`
    /// under the home store, rebuilt from the partitions the control
    /// snapshots name when it is fresh or was recreated (design §5, *Freshness
    /// without a watcher*); a valid one keeps its rows and is checked
    /// partition by partition on first use. A cache that cannot be opened at
    /// all is replaced by an in-memory one with one WARN — the index is
    /// disposable, the daemon is not.
    pub fn open_default(db: Option<Database>) -> Self {
        let index = match MemoryIndex::default_path().and_then(|p| Ok(MemoryIndex::open(&p)?)) {
            Ok(index) => index,
            Err(e) => {
                tracing::warn!("memory index cache unavailable, using an in-memory index: {e:#}");
                MemoryIndex::open_in_memory().expect("an in-memory SQLite index always opens")
            }
        };
        let service = Self::new(db, index);
        match service.index.index_state() {
            Ok(report) if report.state == IndexState::Rebuilding => {
                if let Err(e) = service.rebuild_index() {
                    tracing::warn!("memory index rebuild failed: {e:#}");
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("memory index state unreadable: {e}"),
        }
        service
    }

    pub fn index(&self) -> &MemoryIndex {
        &self.index
    }

    // ------------------------------------------------------------------------
    // Contexts (design §3, §7a)
    // ------------------------------------------------------------------------

    fn owner_exists(&self, id: &str) -> bool {
        let Some(db) = &self.db else {
            return false;
        };
        db.with_connection(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM global_user WHERE id = ?1",
                [id],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        })
        .unwrap_or(false)
    }

    /// Builds a context. `owner_id` must be a `global_user` row (design §3):
    /// the `system` sentinel, a `provider:id` string and any other value
    /// yield [`MemoryError::NoOwner`], and nothing is created for them.
    pub fn context(
        &self,
        owner_id: &str,
        workspace: WorkspaceHint<'_>,
        surface: Surface,
        capabilities: Capabilities,
        attribution: Attribution,
    ) -> Result<MemoryAccessContext, MemoryError> {
        if owner_id.is_empty() || !self.owner_exists(owner_id) {
            return Err(MemoryError::NoOwner(owner_id.to_string()));
        }
        Ok(MemoryAccessContext {
            owner_id: owner_id.to_string(),
            owner_key: files::owner_key(owner_id),
            project_root: resolve_project_root(workspace),
            surface,
            capabilities,
            attribution,
        })
    }

    /// A tool call's context (design §7a, the per-surface table): the owner
    /// is `ToolContext::created_by()` — the principal first, the owner id
    /// second — so a `System` principal and an unlinked `External` one have
    /// no owner; the project root is `request_workspace_root` (R22, never
    /// CWD-derived); the surface is `ModelTool`; the capabilities are what
    /// the tool's registration surface granted.
    pub fn context_for_tool(
        &self,
        ctx: &crate::tools::registry::ToolContext,
        capabilities: Capabilities,
    ) -> Result<MemoryAccessContext, MemoryError> {
        let owner = ctx.created_by();
        let root = ctx.request_workspace_root.as_deref().map(PathBuf::from);
        let hint = match &root {
            Some(r) => WorkspaceHint::Root(r.as_path()),
            None => WorkspaceHint::None,
        };
        self.context(
            &owner,
            hint,
            Surface::ModelTool,
            capabilities,
            Attribution {
                lane_key: ctx.lane_key.clone(),
                task_id: ctx.task_id.clone(),
                request_id: ctx.request_id,
                source: ctx.source.clone(),
            },
        )
    }

    // ------------------------------------------------------------------------
    // Partitions (design §3 rule 1)
    // ------------------------------------------------------------------------

    fn open_partition(scope: &StoreScope, owner_id: &str) -> Result<Partition, MemoryError> {
        Partition::open(scope, owner_id).map_err(MemoryError::from)
    }

    /// The owner's home partition and, when the request resolved a project
    /// root whose path to the partition is clean, the project partition. A
    /// link, a junction or a non-directory on that path refuses the project
    /// partition with one WARN naming the component, and the request falls
    /// back to the global partition (design §3 rule 1).
    pub(super) fn resolve(&self, ctx: &MemoryAccessContext) -> Result<Resolved, MemoryError> {
        let home = Self::open_partition(&StoreScope::Home, &ctx.owner_id)?;
        let project = match &ctx.project_root {
            None => None,
            Some(root) => {
                match Partition::open(&StoreScope::Project(root.clone()), &ctx.owner_id) {
                    Ok(p) => Some(p),
                    Err(MemoryFilesError::PathRefused { component, reason }) => {
                        tracing::warn!(
                            project_root = %root.display(),
                            component = %component.display(),
                            %reason,
                            "memory: project partition refused, using the global partition only"
                        );
                        None
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        };
        Ok(Resolved { home, project })
    }

    fn slot_key(scope: &StoreScope, owner_key: &str) -> String {
        match scope {
            StoreScope::Home => format!("home:{owner_key}"),
            StoreScope::Project(root) => format!("proj:{}", canonical_lenient(root)),
        }
    }

    /// Design §3, *Workspace move*, step 1: refuses while a memory write for
    /// that project is pending; otherwise holds new writes until the guard
    /// drops (step 5).
    pub fn quiesce(&self, project_root: &Path) -> Result<QuiesceGuard, MemoryError> {
        let key = Self::slot_key(&StoreScope::Project(project_root.to_path_buf()), "");
        self.slots
            .quiesce(&key)
            .map_err(|()| MemoryError::WritePending { key })
    }

    /// Design §3, *Workspace move*, step 2: before anything is renamed,
    /// every partition of `old_project` gets its control snapshot written
    /// under the new partition key — bound to the directory the rename will
    /// create, `moved_from` naming the old key — with the old snapshot left
    /// in place ([`files::begin_project_move`]). The caller holds the
    /// [`QuiesceGuard`] meanwhile.
    pub fn begin_project_move(
        &self,
        old_project: &Path,
        new_project: &Path,
    ) -> Result<ProjectMove, MemoryError> {
        let moves = files::begin_project_move(old_project, new_project, Utc::now())?;
        Ok(ProjectMove { moves })
    }

    /// Design §3, *Workspace move*, step 5: deletes the old-key snapshots and
    /// clears `moved_from` on their twins ([`files::finish_project_move`]),
    /// then drops the old keys' rows from the index and this process's
    /// fingerprints — the moved partition is re-indexed under its new key on
    /// its first use, like any partition the index has no rows for. The
    /// caller releases the hold by dropping its guard.
    pub fn finish_project_move(&self, mv: ProjectMove) -> Result<(), MemoryError> {
        files::finish_project_move(&mv.moves, Utc::now())?;
        let mut fingerprints = self.fingerprints.lock().unwrap_or_else(|e| e.into_inner());
        let mut last_scans = self.last_scans.lock().unwrap_or_else(|e| e.into_inner());
        for m in &mv.moves {
            self.index.remove_partition(&m.old_key)?;
            fingerprints.remove(&m.old_key);
            last_scans.remove(&m.old_key);
        }
        Ok(())
    }

    /// The project roots `owner_id`'s control snapshots know (plan 3.7): a
    /// project can be known by its memory partition alone, with no row in
    /// any database member, and the workspace listing adds these to the
    /// roots the database names. Scoped to the owner like every other
    /// workspace answer (design §3, R40): another owner's partition in a
    /// project of its own must not name that root to this one. Only a
    /// snapshot whose partition directory exists counts — one bound to a
    /// missing directory is a stale, abandoned or orphaned move snapshot,
    /// which is `doctor`'s to name — and each root is listed once. Sorted,
    /// as the snapshots record them (canonical). Unreadable snapshots are
    /// skipped.
    pub fn known_project_roots(&self, owner_id: &str) -> Vec<PathBuf> {
        let owner_key = files::owner_key(owner_id);
        let snapshots = match files::list_snapshots() {
            Ok(list) => list,
            Err(e) => {
                tracing::warn!(
                    "memory: control snapshots unreadable, listing no project roots: {e:#}"
                );
                return Vec::new();
            }
        };
        let mut roots: Vec<PathBuf> = snapshots
            .into_iter()
            .filter_map(|(_, parsed)| parsed.ok())
            .filter(|s| s.scope == files::SnapshotScope::Project && s.owner_key == owner_key)
            .filter(|s| Path::new(&s.root).is_dir())
            .filter_map(|s| Path::new(&s.root).ancestors().nth(4).map(Path::to_path_buf))
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }

    // ------------------------------------------------------------------------
    // The index (design §5, *Freshness without a watcher*)
    // ------------------------------------------------------------------------

    fn scan_entries(p: &Partition) -> std::io::Result<Vec<index::ScanEntry>> {
        let entries = p.scan().map_err(std::io::Error::other)?;
        Ok(entries
            .iter()
            .map(|e| index::ScanEntry {
                relative_path: format!("{}/{}", files::NOTES_DIR, e.file_name),
                stat: file_stat(e),
            })
            .collect())
    }

    fn records_from(
        p: &Partition,
        loaded: &LoadedPartition,
        scan: &[ScanEntry],
    ) -> Vec<IndexRecord> {
        let stats: HashMap<&str, FileStat> = scan
            .iter()
            .map(|e| (e.file_name.as_str(), file_stat(e)))
            .collect();
        let scope = match p.scope() {
            StoreScope::Home => NoteScope::Global,
            StoreScope::Project(_) => NoteScope::Project,
        };
        loaded
            .views
            .iter()
            .filter(|v| {
                if v.note.body.len() > MAX_NOTE_BYTES {
                    tracing::warn!(
                        id = %v.note.id,
                        "memory: note body over {MAX_NOTE_BYTES} bytes is left out of the search index"
                    );
                    return false;
                }
                true
            })
            .map(|v| IndexRecord {
                id: v.note.id.to_string(),
                revision: v.revision.clone(),
                partition_key: p.partition_key().to_string(),
                scope: scope.as_str().to_string(),
                relative_path: format!("{}/{}", files::NOTES_DIR, v.file_name),
                title: v.note.title.clone(),
                aliases: v.note.aliases.clone(),
                tags: v.note.tags.clone(),
                kind: v.note.kind.as_str().to_string(),
                status: v.note.status.as_str().to_string(),
                effective_status: map_status(v.derived.effective_status),
                origin: v.derived.origin.as_str().to_string(),
                fact_key: v.note.fact_key.clone(),
                valid_from: v.note.valid_from.map(format_ts),
                valid_to: v.note.valid_to.map(format_ts),
                updated_at: format_ts(v.note.updated_at),
                pinned: v.note.pinned,
                source_forgotten: v.derived.source_forgotten,
                conflict: v.derived.conflict,
                same_key_loser: v.derived.same_key_loser_to.is_some(),
                superseded_by: v.derived.superseded_by.iter().map(|id| id.to_string()).collect(),
                supersedes: v.note.supersedes.iter().map(|id| id.to_string()).collect(),
                sources: v.note.sources.iter().map(NoteSource::suppression_key).collect(),
                body: v.note.body.clone(),
                body_line_offset: body_line_offset(&v.note),
                file: stats.get(v.file_name.as_str()).copied().unwrap_or_default(),
            })
            .collect()
    }

    fn fingerprint(record: &IndexRecord, view: &NoteView) -> Fingerprint {
        Fingerprint {
            revision: record.revision.clone(),
            effective_status: view.derived.effective_status,
            origin: view.derived.origin,
            conflict: record.conflict,
            same_key_loser: record.same_key_loser,
            source_forgotten: record.source_forgotten,
            superseded_by: view.derived.superseded_by.clone(),
            pinned: record.pinned,
            relative_path: record.relative_path.clone(),
            stat: (record.file.mtime_ns, record.file.size, record.file.inode),
        }
    }

    /// Re-indexes a partition from the disk: everything on a first visit or
    /// when the index has no rows for it, otherwise only the notes whose
    /// facts changed since this process last indexed them.
    fn reindex(&self, p: &Partition, full: bool) -> Result<(), MemoryError> {
        let key = p.partition_key().to_string();
        let scan = p.scan()?;
        let loaded = p.load()?;
        let records = Self::records_from(p, &loaded, &scan);
        let mut fingerprints = self.fingerprints.lock().unwrap_or_else(|e| e.into_inner());
        let known = fingerprints.entry(key.clone()).or_default();
        if full || known.is_empty() {
            self.index.replace_partition(&key, &records)?;
            known.clear();
            for (record, view) in records.iter().zip(
                loaded
                    .views
                    .iter()
                    .filter(|v| v.note.body.len() <= MAX_NOTE_BYTES),
            ) {
                known.insert(view.note.id, Self::fingerprint(record, view));
            }
        } else {
            let mut seen: HashSet<NoteId> = HashSet::new();
            let mut changed: Vec<IndexRecord> = Vec::new();
            for (record, view) in records.iter().zip(
                loaded
                    .views
                    .iter()
                    .filter(|v| v.note.body.len() <= MAX_NOTE_BYTES),
            ) {
                seen.insert(view.note.id);
                let fp = Self::fingerprint(record, view);
                if known.get(&view.note.id) != Some(&fp) {
                    changed.push(record.clone());
                    known.insert(view.note.id, fp);
                }
            }
            let gone: Vec<NoteId> = known
                .keys()
                .filter(|id| !seen.contains(id))
                .copied()
                .collect();
            for id in gone {
                known.remove(&id);
                self.index.remove(&key, &id.to_string())?;
            }
            if !changed.is_empty() {
                self.index.upsert_many(&changed)?;
            }
        }
        drop(fingerprints);
        // Stamp the scan so `last_scan` says the rows were checked.
        self.index.reconcile(&key, || Self::scan_entries(p))?;
        Ok(())
    }

    /// Rebuilds the whole cache from every partition a control snapshot
    /// names (a fresh or recreated index). Best effort per partition.
    pub fn rebuild_index(&self) -> Result<(), MemoryError> {
        let mut records: Vec<IndexRecord> = Vec::new();
        let mut opened: Vec<Partition> = Vec::new();
        for (path, parsed) in files::list_snapshots()? {
            let snapshot = match parsed {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(path = %path.display(), "memory: unreadable control snapshot skipped during rebuild: {e}");
                    continue;
                }
            };
            let scope = match snapshot.scope {
                files::SnapshotScope::Home => StoreScope::Home,
                files::SnapshotScope::Project => {
                    let Some(project) = Path::new(&snapshot.root).ancestors().nth(4) else {
                        continue;
                    };
                    StoreScope::Project(project.to_path_buf())
                }
            };
            match Partition::open(&scope, &snapshot.owner_id) {
                Ok(p) => opened.push(p),
                Err(e) => tracing::warn!(
                    partition = %snapshot.partition_key,
                    "memory: partition skipped during rebuild: {e}"
                ),
            }
        }
        for p in &opened {
            let scan = p.scan()?;
            let loaded = p.load()?;
            records.extend(Self::records_from(p, &loaded, &scan));
        }
        self.index.rebuild(&records)?;
        {
            let mut fingerprints = self.fingerprints.lock().unwrap_or_else(|e| e.into_inner());
            fingerprints.clear();
        }
        for p in &opened {
            self.index
                .reconcile(p.partition_key(), || Self::scan_entries(p))?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Reads
    // ------------------------------------------------------------------------

    fn require(ok: bool, capability: &'static str) -> Result<(), MemoryError> {
        if ok {
            Ok(())
        } else {
            Err(MemoryError::CapabilityDenied { capability })
        }
    }

    /// The note `id` and the partition holding it, if any of the owner's
    /// partitions does.
    fn find_note<'a>(
        resolved: &'a Resolved,
        id: &NoteId,
    ) -> Result<Option<(&'a Partition, NoteView)>, MemoryError> {
        for p in resolved.all() {
            if let Some(view) = p.load()?.get(id).cloned() {
                return Ok(Some((p, view)));
            }
        }
        Ok(None)
    }

    /// `memory_read` (design §7a): the current Markdown and provenance, ≤ 32
    /// KiB, from the owner's own partitions only.
    pub fn read(
        &self,
        ctx: &MemoryAccessContext,
        input: ReadInput,
    ) -> Result<ReadOutput, MemoryError> {
        Self::require(ctx.capabilities.read, "memory_read")?;
        let resolved = self.resolve(ctx)?;
        let Some((p, view)) = Self::find_note(&resolved, &input.id)? else {
            return Err(MemoryError::NotFound(input.id));
        };
        if let Some(expected) = &input.expected_revision
            && *expected != view.revision
        {
            return Err(MemoryError::RevisionConflict {
                id: input.id,
                current_revision: view.revision,
            });
        }
        let scope = scope_of(p);
        let freshness = if view.unverified {
            Freshness::Unverified
        } else {
            Freshness::Verified
        };
        let full_body = match &input.section {
            Some(heading) => section_of(&view.note.body, heading).ok_or_else(|| {
                MemoryError::invalid(format!(
                    "note {} has no section headed `{heading}`",
                    input.id
                ))
            })?,
            None => view.note.body.clone(),
        };
        let (body, truncated) = cut_bytes(&full_body, READ_MAX_BYTES);
        let note = &view.note;
        let frontmatter = management::frontmatter_json(note);
        if ctx.surface == Surface::ModelTool {
            self.record_used(
                ctx,
                &[MemoryUsed {
                    id: note.id,
                    revision: view.revision.clone(),
                    reason: UsedReason::Tool,
                }],
            );
        }
        Ok(ReadOutput {
            id: note.id,
            revision: view.revision.clone(),
            title: note.title.clone(),
            kind: note.kind,
            scope,
            status: note.status,
            effective_status: view.derived.effective_status,
            origin: view.derived.origin,
            frontmatter,
            body,
            truncated,
            path: format!("{}/{}", files::NOTES_DIR, view.file_name),
            sources: note.sources.clone(),
            supersedes: note.supersedes.clone(),
            superseded_by: view.derived.superseded_by.clone(),
            freshness,
            conflict: view.derived.conflict,
            source_forgotten: view.derived.source_forgotten,
        })
    }

    /// `memory_list` (design §7a): metadata, `active` by default; superseded
    /// history and candidates only on explicit request; cursor pagination
    /// over `(updated_at, id)`, valid for one filter set.
    pub fn list(
        &self,
        ctx: &MemoryAccessContext,
        input: ListInput,
    ) -> Result<ListOutput, MemoryError> {
        Self::require(ctx.capabilities.read, "memory_read")?;
        let limit = input.limit.unwrap_or(LIST_DEFAULT_LIMIT);
        if limit == 0 || limit > LIST_MAX_LIMIT {
            return Err(MemoryError::invalid(format!(
                "limit must be between 1 and {LIST_MAX_LIMIT}"
            )));
        }
        let filter_fp = list_filter_fingerprint(&input);
        let after: Option<(String, String)> = match &input.cursor {
            None => None,
            Some(cursor) => Some(decode_cursor(cursor, &filter_fp)?),
        };
        let resolved = self.resolve(ctx)?;
        let mut items: Vec<ListItem> = Vec::new();
        for p in resolved.in_scope(input.scope) {
            let scope = scope_of(p);
            for view in p.load()?.views {
                if let Some(kind) = input.kind
                    && view.note.kind != kind
                {
                    continue;
                }
                let effective = view.derived.effective_status;
                let wanted = match input.status.unwrap_or(NoteStatus::Active) {
                    NoteStatus::Active => effective == EffectiveStatus::Active,
                    NoteStatus::Archived => effective == EffectiveStatus::Archived,
                    NoteStatus::Candidate => effective == EffectiveStatus::Candidate,
                } || (input.include_history
                    && effective == EffectiveStatus::Superseded)
                    || (input.include_candidates && effective == EffectiveStatus::Candidate);
                if !wanted {
                    continue;
                }
                items.push(ListItem {
                    id: view.note.id,
                    revision: view.revision,
                    title: view.note.title,
                    kind: view.note.kind,
                    scope,
                    status: view.note.status,
                    effective_status: effective,
                    origin: view.derived.origin,
                    updated_at: view.note.updated_at,
                    created_at: view.note.created_at,
                    pinned: view.note.pinned,
                    supersedes: view.note.supersedes,
                });
            }
        }
        items.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.to_string().cmp(&b.id.to_string()))
        });
        if let Some((ts, id)) = after {
            items.retain(|it| {
                let key = (format_ts(it.updated_at), it.id.to_string());
                // Strictly after the cursor in the sort order above.
                key.0 < ts || (key.0 == ts && key.1 > id)
            });
        }
        let next_cursor = if items.len() > limit {
            items.truncate(limit);
            items
                .last()
                .map(|it| encode_cursor(&format_ts(it.updated_at), &it.id.to_string(), &filter_fp))
        } else {
            None
        };
        Ok(ListOutput { items, next_cursor })
    }

    /// Recently learned (design §6): `extracted_user` notes that automatic
    /// recall can deliver — effectively active, not a same-key loser, no
    /// forgotten source (design 09 §3.2, S7) — newest `created_at` first,
    /// since `since` (default 7 days) and at most `limit`. A learned note that
    /// silently loses to one the user saved is never used, so it is not
    /// listed as learned.
    pub fn recently_learned(
        &self,
        ctx: &MemoryAccessContext,
        since: Option<DateTime<Utc>>,
        limit: Option<usize>,
    ) -> Result<Vec<ListItem>, MemoryError> {
        Self::require(ctx.capabilities.read, "memory_read")?;
        let since = since.unwrap_or_else(|| Utc::now() - chrono::Duration::days(7));
        let limit = limit.unwrap_or(LIST_DEFAULT_LIMIT).clamp(1, LIST_MAX_LIMIT);
        let resolved = self.resolve(ctx)?;
        let mut items = Vec::new();
        for p in resolved.all() {
            let scope = scope_of(p);
            for view in p.load()?.views {
                if view.derived.origin != Origin::ExtractedUser
                    || !view.derived.recallable()
                    || view.note.created_at < since
                {
                    continue;
                }
                items.push(ListItem {
                    id: view.note.id,
                    revision: view.revision,
                    title: view.note.title,
                    kind: view.note.kind,
                    scope,
                    status: view.note.status,
                    effective_status: view.derived.effective_status,
                    origin: view.derived.origin,
                    updated_at: view.note.updated_at,
                    created_at: view.note.created_at,
                    pinned: view.note.pinned,
                    supersedes: view.note.supersedes,
                });
            }
        }
        items.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.to_string().cmp(&b.id.to_string()))
        });
        items.truncate(limit);
        Ok(items)
    }

    // ------------------------------------------------------------------------
    // Writes (design §6)
    // ------------------------------------------------------------------------

    fn verify_sources(&self, sources: &[NoteSource]) -> Result<(), MemoryError> {
        if sources.len() > MAX_SOURCES {
            return Err(MemoryError::invalid(format!(
                "sources exceed {MAX_SOURCES} entries"
            )));
        }
        if sources.is_empty() {
            return Ok(());
        }
        let Some(db) = &self.db else {
            return Err(MemoryError::invalid(
                "sources cannot be verified: this daemon has no database",
            ));
        };
        for source in sources {
            let found = db
                .with_connection(|conn| {
                    let n: i64 = match source.kind {
                        SourceType::Message => {
                            let Ok(rowid) = source.id.parse::<i64>() else {
                                return Ok(0);
                            };
                            conn.query_row(
                                "SELECT COUNT(*) FROM conversation_messages WHERE id = ?1",
                                [rowid],
                                |r| r.get(0),
                            )?
                        }
                        SourceType::Task => conn.query_row(
                            "SELECT COUNT(*) FROM task WHERE id = ?1",
                            [source.id.as_str()],
                            |r| r.get(0),
                        )?,
                    };
                    Ok(n)
                })
                .map_err(MemoryError::Internal)?;
            if found == 0 {
                return Err(MemoryError::invalid(format!(
                    "source {} does not exist",
                    source.suppression_key()
                )));
            }
        }
        Ok(())
    }

    fn validate_store(
        &self,
        ctx: &MemoryAccessContext,
        input: &StoreInput,
    ) -> Result<(), MemoryError> {
        let title_chars = input.title.chars().count();
        if input.title.trim().is_empty() || title_chars > MAX_TITLE_CHARS {
            return Err(MemoryError::invalid(format!(
                "title must be 1 to {MAX_TITLE_CHARS} characters"
            )));
        }
        if input.body.len() > MAX_NOTE_BYTES {
            return Err(MemoryError::invalid(format!(
                "body is {} bytes, above the 32 KiB ({MAX_NOTE_BYTES} byte) note limit",
                input.body.len()
            )));
        }
        if input.aliases.len() > MAX_LIST_ENTRIES || input.tags.len() > MAX_LIST_ENTRIES {
            return Err(MemoryError::invalid(format!(
                "aliases and tags take at most {MAX_LIST_ENTRIES} entries each"
            )));
        }
        if let Some(key) = &input.fact_key
            && !is_valid_fact_key(key)
        {
            return Err(MemoryError::invalid(format!(
                "fact_key `{key}` does not match [a-z0-9][a-z0-9._-]{{0,63}}"
            )));
        }
        if let (Some(from), Some(to)) = (input.valid_from, input.valid_to)
            && to < from
        {
            return Err(MemoryError::invalid("valid_to is before valid_from"));
        }
        if ctx.surface != Surface::Extraction
            && (input.observed_at.is_some() || input.replace_policy != ReplacePolicy::Direct)
        {
            return Err(MemoryError::invalid(
                "observed_at and a replace policy are set by the extractor only",
            ));
        }
        match (ctx.surface, input.provenance) {
            (Surface::Extraction, None) => {
                return Err(MemoryError::invalid(
                    "an extraction write must say whose words it extracted",
                ));
            }
            (Surface::Extraction, Some(_)) => {}
            (_, Some(_)) => {
                return Err(MemoryError::invalid(
                    "provenance is set by the extractor only",
                ));
            }
            (_, None) => {}
        }
        if let Some((_, rev)) = &input.supersedes
            && !files::is_revision(rev)
        {
            return Err(MemoryError::invalid(
                "supersedes.expected_revision is not a 64-character hex revision",
            ));
        }
        self.verify_sources(&input.sources)
    }

    /// `memory_store` (design §7a): a new note in the partition the scope
    /// rule picks, or the exact duplicate's id; with `supersedes`, the
    /// successor of a note in the same partition (design §6).
    pub fn store(
        &self,
        ctx: &MemoryAccessContext,
        input: StoreInput,
    ) -> Result<StoreOutput, MemoryError> {
        Self::require(ctx.capabilities.write, "memory_write")?;
        self.validate_store(ctx, &input)?;
        let Some(kind) = input.kind else {
            return Err(MemoryError::invalid("kind is required"));
        };
        let resolved = self.resolve(ctx)?;
        let target_scope = match (input.scope, resolved.project.is_some()) {
            (Some(NoteScope::Project), false) => {
                return Err(MemoryError::invalid(
                    "scope `project` needs a project, and this request resolved no project root \
                     (no `.openalpaca` or `.git` above its workspace, or none was named)",
                ));
            }
            (Some(scope), _) => scope,
            (None, true) => kind.default_scope(),
            (None, false) => NoteScope::Global,
        };
        let store_scope = match target_scope {
            NoteScope::Global => StoreScope::Home,
            NoteScope::Project => StoreScope::Project(
                ctx.project_root
                    .clone()
                    .expect("a project scope resolved a project partition"),
            ),
        };
        // A `supersedes` target in the *other* partition: the link would be
        // ignored (design §6, same partition), so the store is refused
        // rather than silently creating an unlinked note.
        if let Some((target, _)) = &input.supersedes {
            let other = match target_scope {
                NoteScope::Global => resolved.project.as_ref(),
                NoteScope::Project => Some(&resolved.home),
            };
            if let Some(other) = other
                && other.load()?.get(target).is_some()
            {
                return Err(MemoryError::invalid(format!(
                    "note {target} lives in the {} partition; a replacement is stored in the same scope",
                    scope_of(other)
                )));
            }
        }
        let (status, origin) = match (ctx.surface, input.provenance) {
            (Surface::Extraction, Some(ExtractionProvenance::UserWords)) => {
                (NoteStatus::Active, Some(Origin::ExtractedUser))
            }
            (Surface::Extraction, _) => (NoteStatus::Candidate, Some(Origin::ExtractedOther)),
            _ => (NoteStatus::Active, None),
        };
        let actor = ctx.actor(origin);
        let new = NewNote {
            title: input.title,
            body: input.body,
            kind: Some(kind),
            status: Some(status),
            aliases: input.aliases,
            tags: input.tags,
            fact_key: input.fact_key,
            sources: input.sources,
            observed_at: input.observed_at,
            valid_from: input.valid_from,
            valid_to: input.valid_to,
            supersedes: input.supersedes,
            replace_policy: input.replace_policy,
        };
        let _write = self
            .slots
            .begin_write(&Self::slot_key(&store_scope, &ctx.owner_key));
        let partition = Self::open_partition(&store_scope, &ctx.owner_id)?;
        let created = partition.create(new, actor)?;
        self.reindex(&partition, false)?;
        let change = (created.dedup == Dedup::Created).then(|| MemoryChange {
            partition_key: partition.partition_key().to_string(),
            note_id: created.id,
            change: "created",
            revision: Some(created.revision.clone()),
            origin: created.origin,
            surface: Some(ctx.surface),
        });
        if let Some(change) = &change {
            self.publish(change);
        }
        Ok(StoreOutput {
            id: created.id,
            revision: created.revision,
            scope: target_scope,
            origin: created.origin,
            superseded: created.superseded,
            dedup: created.dedup,
            suggestion: created.suggestion,
            change,
        })
    }

    /// One reconciliation scan now (`POST /v1/memories/sync`, design §5
    /// *Freshness*): the snapshot re-records what changed behind the
    /// service's back, the index follows, and each change is reported as a
    /// `memory_changed` with `change: external` and no surface (§7b P13).
    /// It re-records the snapshot, so it needs the write capability like
    /// every other mutation.
    pub fn sync(&self, ctx: &MemoryAccessContext) -> Result<SyncReport, MemoryError> {
        Self::require(ctx.capabilities.write, "memory_write")?;
        let resolved = self.resolve(ctx)?;
        let mut scanned = 0;
        let mut changes = Vec::new();
        for p in resolved.all() {
            if p.health() != PartitionHealth::Ready {
                continue;
            }
            let _write = self
                .slots
                .begin_write(&Self::slot_key(p.scope(), &ctx.owner_key));
            let fresh = Self::open_partition(p.scope(), &ctx.owner_id)?;
            scanned += fresh.scan()?.len();
            for change in fresh.reconcile()? {
                let change = MemoryChange {
                    partition_key: fresh.partition_key().to_string(),
                    note_id: change.id,
                    change: "external",
                    revision: change.revision,
                    origin: Origin::External,
                    surface: None,
                };
                self.publish(&change);
                changes.push(change);
            }
            self.reindex(&fresh, false)?;
        }
        Ok(SyncReport {
            scanned,
            changes,
            index_state: self.index.index_state()?.state,
        })
    }

    // ------------------------------------------------------------------------
    // The recently-used ring (design §5 step 8)
    // ------------------------------------------------------------------------

    /// [`MemoryService::record_delivery`] under a context's attribution —
    /// what `search` and `read` call on the `ModelTool` surface, where the
    /// tool result *is* the delivery.
    pub fn record_used(&self, ctx: &MemoryAccessContext, used: &[MemoryUsed]) {
        self.record_delivery(&ctx.attribution, used);
    }

    /// Records a delivery (design §5 step 8): into the ring that
    /// `recently_used` reads and, when the attribution names a request, into
    /// that request's `memory_used`. The caller records *after* the notes are
    /// in front of a model — `recall` selects, it does not deliver — so the
    /// turn reports what the model really read. A note already recorded for
    /// the request under the same revision and reason is not repeated.
    pub fn record_delivery(&self, attribution: &Attribution, used: &[MemoryUsed]) {
        if used.is_empty() {
            return;
        }
        let now = Utc::now();
        {
            let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
            for item in used {
                if ring.len() >= RING_CAPACITY {
                    ring.pop_front();
                }
                ring.push_back(RecentlyUsedItem {
                    id: item.id,
                    revision: item.revision.clone(),
                    reason: item.reason,
                    delivered_at: now,
                    lane_key: attribution.lane_key.clone(),
                    task_id: attribution.task_id.clone(),
                });
            }
        }
        let Some(request_id) = attribution.request_id else {
            return;
        };
        let mut by_request = self
            .used_by_request
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let slot = match by_request.iter().position(|(id, _)| *id == request_id) {
            Some(i) => i,
            None => {
                if by_request.len() >= USED_REQUESTS_CAPACITY {
                    by_request.pop_front();
                }
                by_request.push_back((request_id, Vec::new()));
                by_request.len() - 1
            }
        };
        let entry = &mut by_request[slot].1;
        for item in used {
            if !entry.contains(item) {
                entry.push(item.clone());
            }
        }
    }

    /// The request's `memory_used` (design §7a) in delivery order — the
    /// automatic recall first, then what the model fetched through
    /// `memory_search` / `memory_read` — taken once: a second call, or a
    /// request that delivered nothing, answers empty.
    pub fn take_used(&self, request_id: uuid::Uuid) -> Vec<MemoryUsed> {
        let mut by_request = self
            .used_by_request
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match by_request.iter().position(|(id, _)| *id == request_id) {
            Some(i) => by_request.remove(i).map(|(_, used)| used).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// The newest deliveries first, at most `limit` (≤ 200). Cleared at
    /// restart.
    pub fn recently_used(&self, limit: usize) -> Vec<RecentlyUsedItem> {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.iter()
            .rev()
            .take(limit.clamp(1, RING_CAPACITY))
            .cloned()
            .collect()
    }
}

// ============================================================================
// Helpers
// ============================================================================

pub(super) fn scope_of(p: &Partition) -> NoteScope {
    match p.scope() {
        StoreScope::Home => NoteScope::Global,
        StoreScope::Project(_) => NoteScope::Project,
    }
}

fn canonical_lenient(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn file_stat(e: &ScanEntry) -> FileStat {
    FileStat {
        mtime_ns: e
            .mtime
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0),
        size: e.size,
        inode: e.inode,
    }
}

fn map_status(s: EffectiveStatus) -> index::EffectiveStatus {
    match s {
        EffectiveStatus::Active => index::EffectiveStatus::Active,
        EffectiveStatus::Superseded => index::EffectiveStatus::Superseded,
        EffectiveStatus::Archived => index::EffectiveStatus::Archived,
        EffectiveStatus::Candidate => index::EffectiveStatus::Candidate,
    }
}

/// RFC 3339 UTC to the second with `Z` — the canon's one timestamp form.
pub(super) fn format_ts(ts: DateTime<Utc>) -> String {
    ts.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// How many file lines precede the body's first line, from the canonical
/// serialization (exact for a service-written file).
fn body_line_offset(note: &files::Note) -> u32 {
    let serialized = note.serialize();
    let prefix_len = serialized.len().saturating_sub(note.body.len());
    serialized[..prefix_len].matches('\n').count() as u32
}

pub(super) fn source_json(s: &NoteSource) -> serde_json::Value {
    serde_json::json!({
        "type": s.kind.as_str(),
        "id": s.id,
        "observed_at": s.observed_at.map(format_ts),
    })
}

/// Cuts `text` to at most `max` bytes on a char boundary.
pub(super) fn cut_bytes(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// The section headed `heading` (any level): the heading line and the lines
/// until the next heading of the same or a higher level.
pub(super) fn section_of(body: &str, heading: &str) -> Option<String> {
    let wanted = heading.trim().trim_start_matches('#').trim();
    let lines: Vec<&str> = body.lines().collect();
    let mut start = None;
    let mut level = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if hashes == 0 || hashes > 6 {
            continue;
        }
        let text = line[hashes..].trim();
        if start.is_none() {
            if text.eq_ignore_ascii_case(wanted) {
                start = Some(i);
                level = hashes;
            }
        } else if hashes <= level {
            return Some(lines[start?..i].join("\n"));
        }
    }
    start.map(|s| lines[s..].join("\n"))
}

fn list_filter_fingerprint(input: &ListInput) -> String {
    let text = format!(
        "{:?}|{:?}|{:?}|{}|{}",
        input.kind, input.status, input.scope, input.include_history, input.include_candidates
    );
    files::sha256_hex(text.as_bytes())[..8].to_string()
}

fn encode_cursor(updated_at: &str, id: &str, filter_fp: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(format!("{updated_at}|{id}|{filter_fp}"))
}

fn decode_cursor(cursor: &str, filter_fp: &str) -> Result<(String, String), MemoryError> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| MemoryError::invalid("cursor is not one this listing issued"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| MemoryError::invalid("cursor is not one this listing issued"))?;
    let mut parts = text.splitn(3, '|');
    let (Some(ts), Some(id), Some(fp)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(MemoryError::invalid(
            "cursor is not one this listing issued",
        ));
    };
    if fp != filter_fp {
        return Err(MemoryError::invalid(
            "cursor belongs to a listing with different filters",
        ));
    }
    Ok((ts.to_string(), id.to_string()))
}
