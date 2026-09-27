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
mod timeline;
mod types;
pub mod wire;
mod write_slot;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod episode_tests;
#[cfg(test)]
mod scale_tests;
#[cfg(test)]
mod tests;

pub use context::{
    ATTENDED_CHAT_SOURCES, Attribution, Capabilities, ChatCommandTurn, MemoryAccessContext,
    Surface, WorkspaceHint, resolve_project_root,
};
pub use error::MemoryError;
pub use in_chat::{
    PENDING_SUGGESTIONS_READ_MAX, PREVIEW_MAX_BYTES, PendingSuggestion,
    SUGGESTION_LINE_TITLE_BYTES, SuggestionPreview,
};
pub use openalpaca_storage::memory_index::SearchOrder;
pub use recall::{INDEX_BLOCK_HEADER, TRUST_LABEL, render_delivery, wrap_delivery};
pub use suggestions::{
    SUGGESTION_EXCERPT_BYTES, SUGGESTIONS_DEFAULT_LIMIT, SUGGESTIONS_MAX_LIMIT, suggestion_hidden,
};
pub use types::{
    ExtractionProvenance, ForgetInput, ForgetItem, ForgetItemStatus, ForgetOutput, Freshness,
    HistoryEntry, IndexRow, KnownNote, LearningView, ListInput, ListItem, ListOutput, ListedNote,
    MemoryChange, MemoryUsed, Patch, ReadInput, ReadManyInput, ReadManyItem, ReadManyOutput,
    ReadOutput, RecallBudget, RecallResult, RecalledNote, RecentlyUsedItem, Retained, RowStatus,
    SearchDetail, SearchInput, SearchOutput, SearchResultItem, StoreInput, StoreOutput, Suggestion,
    SuggestionTarget, TimelineAnchor, TimelineInput, TimelineOutput, Timing, UpdateAction,
    UpdateInput, UpdateOutput, UsedReason, WithdrawOutput, match_reasons,
};
pub use write_slot::QuiesceGuard;

use chrono::{DateTime, Utc};
use openalpaca_storage::Database;
use openalpaca_storage::memory_files::{
    self as files, Dedup, EffectiveStatus, LoadedPartition, MAX_LIST_ENTRIES, MAX_NOTE_BYTES,
    MAX_SOURCES, MAX_TITLE_CHARS, MemoryFilesError, NewNote, NoteId, NoteParseCache, NoteScope,
    NoteSource, NoteStatus, NoteView, Origin, Partition, PartitionHealth, ReplacePolicy, ScanEntry,
    SourceType, is_valid_fact_key,
};
use openalpaca_storage::memory_index::{
    self as index, FileStat, IndexRecord, IndexState, MemoryIndex,
};
use openalpaca_storage::store::StoreScope;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::UNIX_EPOCH;

use write_slot::WriteSlots;

/// Design §4 *Initial limits*: the combined automatic memory context.
pub const RECALL_TOTAL_BYTES: usize = 8 * 1024;
/// Design §5 step 2: the pinned sub-budget.
pub const RECALL_PINNED_BYTES: usize = 2 * 1024;
/// Design §4: at most 8 retrieved chunks.
pub const RECALL_MAX_ITEMS: usize = 8;
/// Design 09 §13.2: `recall_mode = "index"` excerpts the top 2 recalled
/// notes…
pub const RECALL_INDEX_EXCERPTS: usize = 2;
/// …and lists at most 8 further ones, one line each.
pub const RECALL_INDEX_LINES: usize = 8;
/// Design §4: the explicit tool read response — one note's, or a batch's
/// together (design 09 §13.1).
pub const READ_MAX_BYTES: usize = 32 * 1024;
/// Design 09 §13.1: `memory_read` takes 1–10 ids.
pub const READ_MAX_IDS: usize = 10;
/// Design 09 §13.1: `memory_search.limit` is 1–50, default 10.
pub const SEARCH_DEFAULT_LIMIT: usize = 10;
pub const SEARCH_MAX_LIMIT: usize = index::MAX_LIMIT;
/// Design 09 §13.1: `memory_timeline.before` and `after` are 0–20, default 5.
pub const TIMELINE_DEFAULT_SIDE: usize = 5;
pub const TIMELINE_MAX_SIDE: usize = 20;
/// `GET /v1/memories/recently-used`: `limit` max 200, the ring's capacity.
pub const RING_CAPACITY: usize = 200;
/// How many requests' deliveries are held for [`MemoryService::take_used`]
/// at once: a request that is never drained is forgotten once this many
/// newer ones have recorded a delivery.
pub const USED_REQUESTS_CAPACITY: usize = 256;
/// `memory_list.limit`: 1–100, default 50.
pub const LIST_DEFAULT_LIMIT: usize = 50;
pub const LIST_MAX_LIMIT: usize = 100;
/// The shortest id prefix [`MemoryService::find_by_id_prefix`] takes: 8 hex
/// characters, the first group of a UUID (design 09 §12.1).
pub const ID_PREFIX_MIN_HEX: usize = 8;
/// Scale slice S5: a cache file grown past this many times its size right
/// after its last rebuild is rebuilt (from a new file) by the reconciliation
/// tick. Measured on 2,000 notes of ~1 KB (release build): editing a quarter
/// of the notes six times over grows the file to 1.39× — segment merges
/// reuse pages but never shrink the file — a whole-partition rewrite to
/// 1.80×, and doubling the notes to 1.78× of the first rebuild (a clean
/// rebuild of the doubled notes is 1.78× too). So at 2× a cache is rebuilt
/// about once per doubling of its content or per three rewrites of every
/// note — geometric, so the rebuilds' cost stays proportional to the
/// writes — and never holds more than its own size again in dead pages.
pub const INDEX_BLOAT_FACTOR: u64 = 2;
/// How long one idle maintenance pass may spend merging FTS segments; each
/// merge step holds the index only briefly (scale slice S5).
pub const INDEX_MAINTENANCE_BUDGET: std::time::Duration = std::time::Duration::from_millis(250);
/// Design §5: while the index is rebuilt, recall and search read the notes
/// from their files — at most this many, the most recently updated first
/// (a scan that leaves notes out says `partial`). Scale slice S6: the
/// realistic store after a year or two is 10K–30K notes, and the bound
/// keeps the scan's matching work to that of a 10K-note partition; the
/// partitions are read whole (through the parse cache) for their
/// derivation either way.
pub const FILE_SCAN_MAX_NOTES: usize = 10_000;

#[cfg(test)]
thread_local! {
    /// How many `IndexRecord`s this thread has built — each one a copy of a
    /// note's body (design 09 §6.3 quick win 4). Read by tests only.
    pub(super) static RECORDS_BUILT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

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

/// The whole-index rebuild (design §5; scale slice S6).
#[derive(Debug, Default)]
struct RebuildState {
    /// The index lacks notes: a fresh, recreated or schema-bumped cache, a
    /// rebuild that failed, or damage met on use. Until a rebuild
    /// completes, recall and search read the files.
    pending: bool,
    /// A rebuild is running: the index is being refilled from nothing.
    running: bool,
}

/// When the index was last seen written and last maintained (scale slice
/// S5), in its own count of row writes.
#[derive(Debug, Default)]
struct MaintenanceMarks {
    /// The count the previous tick saw.
    seen: u64,
    /// The count the last maintenance pass ran at.
    done: u64,
}

/// What a search reads while the index is rebuilt: the notes, from their
/// files, as the index would have held them.
struct FileScan {
    records: Vec<IndexRecord>,
    pinned: Vec<IndexRecord>,
    truncated: bool,
}

/// Tests: points where a test can hold the service still — the rebuild
/// once it has emptied the index, and a reply-path read (recall, the
/// pending suggestions) before it starts.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct TestGates {
    pub rebuild: std::sync::RwLock<()>,
    pub reads: std::sync::RwLock<()>,
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
    /// The parsed notes and control snapshots of the partitions this
    /// service reads (design 09 §6.3), handed to every partition it opens.
    parse_cache: Arc<NoteParseCache>,
    ring: Mutex<VecDeque<RecentlyUsedItem>>,
    /// Per request, what it delivered so far in delivery order — the turn's
    /// `memory_used` (design §7a), taken once by the bridge. Oldest first,
    /// bounded by [`USED_REQUESTS_CAPACITY`].
    used_by_request: Mutex<VecDeque<(uuid::Uuid, Vec<MemoryUsed>)>>,
    /// Per lane, the suggestions a `memory_confirm` card was raised for since
    /// the daemon started, and the notes they would replace (design 09 §4.5):
    /// kept out of that lane's `<memory_suggestions>` so a timed-out or
    /// denied card is not raised again on that lane — in any conversation on
    /// it — nor another restatement of the same change, until the daemon
    /// restarts. In memory only. `memory_confirm`'s description tells the
    /// model the same rule: raise only what is listed now.
    raised: Mutex<HashMap<String, HashSet<NoteId>>>,
    /// Where `memory_changed` goes (design §7a): every write the service
    /// makes, whichever surface asked, and every change a reconciliation
    /// scan detects. `None` in a test that has no bus; the daemon attaches
    /// its bus with [`MemoryService::with_bus`].
    bus: Option<crate::bus::EventBus>,
    /// Whether a whole rebuild is owed or running (scale slice S6), and
    /// the signal a reader waiting for one to end is woken by.
    rebuild: Mutex<RebuildState>,
    rebuilt: Condvar,
    maintenance: Mutex<MaintenanceMarks>,
    #[cfg(test)]
    pub(crate) gates: Arc<TestGates>,
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
            parse_cache: Arc::new(NoteParseCache::default()),
            ring: Mutex::new(VecDeque::with_capacity(RING_CAPACITY)),
            used_by_request: Mutex::new(VecDeque::new()),
            raised: Mutex::new(HashMap::new()),
            bus: None,
            rebuild: Mutex::new(RebuildState::default()),
            rebuilt: Condvar::new(),
            maintenance: Mutex::new(MaintenanceMarks::default()),
            #[cfg(test)]
            gates: Arc::new(TestGates::default()),
        }
    }

    /// Replaces the parse cache (design 09 §6.3) — a bench measures with
    /// `NoteParseCache::new(0)`, which caches nothing; a test injects its
    /// stat function. Before the first use of the service.
    pub fn with_parse_cache(mut self, cache: NoteParseCache) -> Self {
        self.parse_cache = Arc::new(cache);
        self
    }

    /// The parse cache every partition this service opens reads through.
    pub fn parse_cache(&self) -> &NoteParseCache {
        &self.parse_cache
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

    /// The service over the cache at `state/cache/memory-index-v1.sqlite`
    /// under the home store, **ready**: a fresh or recreated cache is
    /// rebuilt from the partitions the control snapshots name before this
    /// returns ([`Self::open_default_deferred`], then
    /// [`Self::run_pending_rebuild`]). What a caller that needs the index at
    /// once uses — a test, the evaluation harness; the daemon opens with
    /// [`Self::open_default_deferred`] and rebuilds in the background.
    pub fn open_default(db: Option<Database>) -> Self {
        let service = Self::open_default_deferred(db);
        if let Err(e) = service.run_pending_rebuild() {
            tracing::warn!("memory index rebuild failed: {e:#}");
        }
        service
    }

    /// The daemon's service (design §5, *Freshness without a watcher*; scale
    /// slice S6): the cache at `state/cache/memory-index-v1.sqlite` under the
    /// home store, opened **without waiting for anything** — no integrity
    /// check, and a fresh, recreated or schema-bumped cache is not rebuilt
    /// here but left owed ([`Self::rebuild_pending`]) for
    /// [`Self::run_pending_rebuild`] to do off the request path. Until it
    /// has, recall and search read the files (design §5: "search meanwhile
    /// serves a bounded scoped file scan with `index_state: rebuilding`").
    /// A valid cache keeps its rows and is checked partition by partition on
    /// first use. A cache that cannot be opened at all is replaced by an
    /// in-memory one with one WARN — the index is disposable, the daemon is
    /// not.
    pub fn open_default_deferred(db: Option<Database>) -> Self {
        let index = match MemoryIndex::default_path().and_then(|p| Ok(MemoryIndex::open(&p)?)) {
            Ok(index) => index,
            Err(e) => {
                tracing::warn!("memory index cache unavailable, using an in-memory index: {e:#}");
                MemoryIndex::open_in_memory().expect("an in-memory SQLite index always opens")
            }
        };
        let service = Self::new(db, index);
        let owed = match service.index.state() {
            Ok(state) => state == IndexState::Rebuilding,
            Err(e) => {
                tracing::warn!("memory index state unreadable, rebuilding it: {e}");
                true
            }
        };
        if owed {
            service.rebuild_state().pending = true;
        }
        service
    }

    fn rebuild_state(&self) -> std::sync::MutexGuard<'_, RebuildState> {
        self.rebuild.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a whole rebuild of the index is owed or running (scale slice
    /// S6): recall and search then read the files, and the suggestions a
    /// turn is told about are left out.
    pub fn rebuild_pending(&self) -> bool {
        let state = self.rebuild_state();
        state.pending || state.running
    }

    /// Whether a whole rebuild is running now.
    pub fn rebuild_running(&self) -> bool {
        self.rebuild_state().running
    }

    /// Whether reads can be served from the index: no rebuild owed or
    /// running.
    fn index_serves(&self) -> bool {
        !self.rebuild_pending()
    }

    /// Blocks while a whole rebuild runs — what a read of the index that
    /// has no file-scan answer (learning's lookups, the timeline, the
    /// suggestions list) does, rather than read an index being refilled.
    fn wait_for_rebuild(&self) {
        let mut state = self.rebuild_state();
        while state.running {
            state = self
                .rebuilt
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    /// What an error from the index means for it: damage to the cache file
    /// (open no longer checks it, scale slice S6) owes a rebuild, which the
    /// next reconciliation tick runs; until then recall and search read the
    /// files. One WARN, with no note text.
    fn note_index_failure(&self, e: &MemoryError) {
        if !e.is_index_corruption() {
            return;
        }
        let mut state = self.rebuild_state();
        if !state.pending {
            state.pending = true;
            tracing::warn!(
                "memory: the search index cache is damaged ({e}); recall and search read the \
                 note files until the next reconciliation rebuilds it"
            );
        }
    }

    /// Runs the whole rebuild the index is owed, if any and none is running
    /// — off the request path: the daemon calls this on a blocking worker
    /// when it starts, and every reconciliation tick calls it. Answers
    /// whether it rebuilt. A rebuild that fails stays owed.
    pub fn run_pending_rebuild(&self) -> Result<bool, MemoryError> {
        {
            let mut state = self.rebuild_state();
            if !state.pending || state.running {
                return Ok(false);
            }
            state.running = true;
        }
        self.finish_rebuild_run(self.rebuild_index_inner())
            .map(|()| true)
    }

    /// The index's upkeep when it is idle (scale slice S5, M1), called by
    /// every reconciliation tick: once a tick finds no index write since the
    /// tick before — and some since the last pass — the FTS segments the
    /// writes left are merged, within [`INDEX_MAINTENANCE_BUDGET`], and the
    /// WAL folded back and truncated; and a cache file grown past
    /// [`INDEX_BLOAT_FACTOR`] times its size after its last rebuild is
    /// rebuilt from a new file, here, off the request path (recall and
    /// search read the files meanwhile). Nothing while a rebuild is owed.
    fn maintain_index_when_idle(&self) {
        if !self.index_serves() {
            return;
        }
        let writes = self.index.writes();
        {
            let mut marks = self
                .maintenance
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let idle = writes == marks.seen;
            marks.seen = writes;
            if !idle || writes == marks.done {
                return;
            }
            marks.done = writes;
        }
        let pass = match self.index.maintain(INDEX_MAINTENANCE_BUDGET) {
            Ok(pass) => pass,
            Err(e) => {
                let e = MemoryError::from(e);
                self.note_index_failure(&e);
                tracing::warn!("memory: index maintenance failed: {e}");
                return;
            }
        };
        if pass.bytes > pass.rebuilt_bytes.saturating_mul(INDEX_BLOAT_FACTOR) {
            tracing::info!(
                bytes = pass.bytes,
                rebuilt_bytes = pass.rebuilt_bytes,
                "memory: the search index cache outgrew twice its rebuilt size; rebuilding it"
            );
            self.rebuild_state().pending = true;
            if let Err(e) = self.run_pending_rebuild() {
                tracing::warn!(
                    "memory: the search index could not be rebuilt; recall and search read \
                     the note files meanwhile, and the next tick tries again: {e}"
                );
            }
        }
    }

    /// Ends a rebuild run: owed again if it failed, and every reader
    /// waiting for it woken.
    fn finish_rebuild_run(&self, result: Result<(), MemoryError>) -> Result<(), MemoryError> {
        let mut state = self.rebuild_state();
        state.running = false;
        state.pending = result.is_err();
        self.rebuilt.notify_all();
        result
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

    /// Opens a partition through the service's parse cache (design 09
    /// §6.3), so an unchanged note file is not read again.
    fn open_partition(&self, scope: &StoreScope, owner_id: &str) -> Result<Partition, MemoryError> {
        self.open_cached(scope, owner_id).map_err(MemoryError::from)
    }

    fn open_cached(
        &self,
        scope: &StoreScope,
        owner_id: &str,
    ) -> Result<Partition, MemoryFilesError> {
        Partition::open_cached(scope, owner_id, &self.parse_cache)
    }

    /// The owner's home partition and, when the request resolved a project
    /// root whose path to the partition is clean, the project partition. A
    /// link, a junction or a non-directory on that path refuses the project
    /// partition with one WARN naming the component, and the request falls
    /// back to the global partition (design §3 rule 1).
    pub(super) fn resolve(&self, ctx: &MemoryAccessContext) -> Result<Resolved, MemoryError> {
        let home = self.open_partition(&StoreScope::Home, &ctx.owner_id)?;
        let project = match &ctx.project_root {
            None => None,
            Some(root) => {
                match self.open_cached(&StoreScope::Project(root.clone()), &ctx.owner_id) {
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

    /// The partition keys a context reaches — its home partition and, when
    /// it resolved one, its project's — derived without opening either.
    /// What a log line about the context's memory names (scale slice S7).
    pub fn partition_keys(&self, ctx: &MemoryAccessContext) -> Vec<String> {
        let mut keys = vec![files::home_partition_key(&ctx.owner_key)];
        if let Some(root) = &ctx.project_root
            && let Ok(key) =
                files::partition_key_for(&StoreScope::Project(root.clone()), &ctx.owner_key)
        {
            keys.push(key);
        }
        keys
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
    /// fingerprints, scans and parse cache entries (design 09 §6.3) — the
    /// moved partition is re-indexed under its new key on its first use,
    /// like any partition the index has no rows for. The caller releases the
    /// hold by dropping its guard.
    pub fn finish_project_move(&self, mv: ProjectMove) -> Result<(), MemoryError> {
        files::finish_project_move(&mv.moves, Utc::now())?;
        let mut fingerprints = self.fingerprints.lock().unwrap_or_else(|e| e.into_inner());
        let mut last_scans = self.last_scans.lock().unwrap_or_else(|e| e.into_inner());
        for m in &mv.moves {
            self.index.remove_partition(&m.old_key)?;
            fingerprints.remove(&m.old_key);
            last_scans.remove(&m.old_key);
            self.parse_cache.evict_partition(&m.old_key);
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

    /// The index row of one note (design §5), a copy of its body.
    fn record_for(p: &Partition, v: &NoteView, file: FileStat) -> IndexRecord {
        #[cfg(test)]
        RECORDS_BUILT.with(|n| n.set(n.get() + 1));
        IndexRecord {
            id: v.note.id.to_string(),
            revision: v.revision.clone(),
            partition_key: p.partition_key().to_string(),
            scope: scope_of(p).as_str().to_string(),
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
            created_at: format_ts(v.note.created_at),
            observed_at: v.note.observed_at.map(format_ts),
            pinned: v.note.pinned,
            source_forgotten: v.derived.source_forgotten,
            conflict: v.derived.conflict,
            same_key_loser: v.derived.same_key_loser_to.is_some(),
            superseded_by: v
                .derived
                .superseded_by
                .iter()
                .map(|id| id.to_string())
                .collect(),
            supersedes: v.note.supersedes.iter().map(|id| id.to_string()).collect(),
            sources: v
                .note
                .sources
                .iter()
                .map(NoteSource::suppression_key)
                .collect(),
            body: v.note.body.clone(),
            body_line_offset: body_line_offset(&v.note),
            file,
        }
    }

    /// The notes of `loaded` the index takes, each with its file's stat: a
    /// body over the note limit is left out, with one WARN per re-index.
    fn indexable(loaded: &LoadedPartition) -> Vec<(&NoteView, FileStat)> {
        let stats: HashMap<&str, FileStat> = loaded
            .scan
            .iter()
            .map(|e| (e.file_name.as_str(), file_stat(e)))
            .collect();
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
            .map(|v| {
                let stat = stats.get(v.file_name.as_str()).copied().unwrap_or_default();
                (v, stat)
            })
            .collect()
    }

    fn records_from(p: &Partition, loaded: &LoadedPartition) -> Vec<IndexRecord> {
        Self::indexable(loaded)
            .into_iter()
            .map(|(v, stat)| Self::record_for(p, v, stat))
            .collect()
    }

    /// What the index needs to know about a note — the facts its row is
    /// built from — read off the view without building the row.
    fn fingerprint(view: &NoteView, file: FileStat) -> Fingerprint {
        Fingerprint {
            revision: view.revision.clone(),
            effective_status: view.derived.effective_status,
            origin: view.derived.origin,
            conflict: view.derived.conflict,
            same_key_loser: view.derived.same_key_loser_to.is_some(),
            source_forgotten: view.derived.source_forgotten,
            superseded_by: view.derived.superseded_by.clone(),
            pinned: view.note.pinned,
            relative_path: format!("{}/{}", files::NOTES_DIR, view.file_name),
            stat: (file.mtime_ns, file.size, file.inode),
        }
    }

    /// Re-indexes a partition from the disk: only the notes whose facts
    /// differ from what the index holds for them.
    fn reindex(&self, p: &Partition) -> Result<(), MemoryError> {
        let loaded = p.load()?;
        self.reindex_view(p, &loaded)
    }

    /// [`Self::reindex`] from a view already read — the one a mutation
    /// returns (design 09 §6.3 quick win 3). The pass builds a row, and
    /// copies a body, only for a note whose fingerprint changed (quick win
    /// 4), and removes the rows of notes that are gone.
    ///
    /// **Exact without a whole re-index** (scale slice S1, O1): the
    /// fingerprints a partition is compared against are, on its first pass
    /// in this process, read from the index's own rows — every fingerprint
    /// field is a `note` column ([`index::RowFacts`]) — so a restart, the
    /// first tick, a stat difference before a read and a dirty hit each
    /// re-index what changed, never the whole partition (46 s at 100K notes,
    /// holding the index). A partition the index has no rows for is simply
    /// indexed note by note. A pass that fails part-way may have recorded
    /// fingerprints of rows it never wrote, so it forgets the partition's
    /// fingerprints and the next pass reads the rows again. The whole
    /// rebuild stays where it is needed: a missing, recreated or
    /// schema-bumped cache ([`Self::rebuild_index`]).
    ///
    /// While a whole rebuild runs the pass is skipped (scale slice S6): the
    /// rebuild reads every partition itself, and a write it read too early
    /// is found by the next read's stat scan or the next write's pass,
    /// whose fingerprints come from the rebuilt rows.
    fn reindex_view(&self, p: &Partition, loaded: &LoadedPartition) -> Result<(), MemoryError> {
        if self.rebuild_running() {
            return Ok(());
        }
        let result = self.reindex_view_once(p, loaded);
        if let Err(e) = &result {
            self.fingerprints
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(p.partition_key());
            self.note_index_failure(e);
        }
        result
    }

    /// The fingerprints of a partition's rows as the index holds them
    /// (scale slice S1), and the ids of rows no fingerprint can stand for
    /// — an id that does not parse — which the caller removes. A row whose
    /// status or origin does not parse gets a fingerprint no note matches,
    /// so it is written again.
    fn fingerprints_from_rows(
        &self,
        key: &str,
    ) -> Result<(HashMap<NoteId, Fingerprint>, Vec<String>), MemoryError> {
        let mut known = HashMap::new();
        let mut strays = Vec::new();
        for row in self.index.row_facts(key)? {
            let Ok(id) = NoteId::parse(&row.id) else {
                strays.push(row.id);
                continue;
            };
            let effective_status = row.effective_status.map(|s| match s {
                index::EffectiveStatus::Active => EffectiveStatus::Active,
                index::EffectiveStatus::Superseded => EffectiveStatus::Superseded,
                index::EffectiveStatus::Archived => EffectiveStatus::Archived,
                index::EffectiveStatus::Candidate => EffectiveStatus::Candidate,
            });
            let origin = Origin::parse(&row.origin);
            let superseded_by: Option<Vec<NoteId>> = row
                .superseded_by
                .iter()
                .map(|s| NoteId::parse(s).ok())
                .collect();
            let readable = effective_status.is_some() && origin.is_some() && superseded_by.is_some();
            known.insert(
                id,
                Fingerprint {
                    // An empty revision matches no file: the row is rewritten.
                    revision: if readable { row.revision } else { String::new() },
                    effective_status: effective_status.unwrap_or(EffectiveStatus::Active),
                    origin: origin.unwrap_or(Origin::External),
                    conflict: row.conflict,
                    same_key_loser: row.same_key_loser,
                    source_forgotten: row.source_forgotten,
                    superseded_by: superseded_by.unwrap_or_default(),
                    pinned: row.pinned,
                    relative_path: row.relative_path,
                    stat: (row.file.mtime_ns, row.file.size, row.file.inode),
                },
            );
        }
        Ok((known, strays))
    }

    /// Step 4 of a mutation (design §6): the index follows a write that is
    /// already durable — the note file and its snapshot entry are the
    /// authority, and the index is a disposable cache (§2). So an index that
    /// fails here does not fail the write: a caller that retried an error
    /// would store a duplicate or meet a revision conflict. The failure is
    /// logged, [`Self::reindex_view`] has dropped the partition's
    /// fingerprints, and the rows the index lacks are found by the next
    /// search's stat scan or the next reconciliation tick (§5, 30 s), which
    /// re-index the partition whole.
    fn reindex_after_write(&self, p: &Partition, loaded: &LoadedPartition) {
        if let Err(e) = self.reindex_view(p, loaded) {
            tracing::warn!(
                partition = p.partition_key(),
                "memory: the write landed, but the search index did not follow; \
                 the next search or reconciliation re-indexes the partition: {e}"
            );
        }
    }

    fn reindex_view_once(&self, p: &Partition, loaded: &LoadedPartition) -> Result<(), MemoryError> {
        let key = p.partition_key().to_string();
        let notes = Self::indexable(loaded);
        let mut fingerprints = self.fingerprints.lock().unwrap_or_else(|e| e.into_inner());
        if !fingerprints.contains_key(&key) {
            let (seeded, strays) = self.fingerprints_from_rows(&key)?;
            for id in strays {
                self.index.remove(&key, &id)?;
            }
            fingerprints.insert(key.clone(), seeded);
        }
        let known = fingerprints.get_mut(&key).expect("seeded above");
        let mut seen: HashSet<NoteId> = HashSet::new();
        let mut changed: Vec<IndexRecord> = Vec::new();
        for (view, stat) in &notes {
            seen.insert(view.note.id);
            let fp = Self::fingerprint(view, *stat);
            if known.get(&view.note.id) != Some(&fp) {
                changed.push(Self::record_for(p, view, *stat));
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
        drop(fingerprints);
        // Stamp the scan so `last_scan` says the rows were checked — the
        // view's own scan, which after a write carries the written file. The
        // rows were just brought in line with it, so nothing is compared
        // (scale slice S4, O10: a reconcile here read every row back).
        self.index.stamp_scan(&key, loaded.scan.len())?;
        Ok(())
    }

    /// Rebuilds the whole cache from every partition a control snapshot
    /// names (a fresh or recreated index), now; one already running is
    /// waited for first. Best effort per partition.
    pub fn rebuild_index(&self) -> Result<(), MemoryError> {
        {
            let mut state = self.rebuild_state();
            while state.running {
                state = self
                    .rebuilt
                    .wait(state)
                    .unwrap_or_else(|e| e.into_inner());
            }
            state.running = true;
        }
        self.finish_rebuild_run(self.rebuild_index_inner())
    }

    /// The rebuild itself (scale slice S6): the cache recreated empty, then
    /// each partition read and indexed in batches — the index is released
    /// between them, and nothing else writes it meanwhile ([`Self::reindex_view`]
    /// skips while a rebuild runs; a write it missed is found by the next
    /// read's stat scan) — then merged and marked ready.
    fn rebuild_index_inner(&self) -> Result<(), MemoryError> {
        // Everything is read again from the disk, as on a fresh start.
        self.parse_cache.clear();
        self.index.begin_rebuild()?;
        #[cfg(test)]
        let _paused = self.gates.rebuild.read().unwrap_or_else(|e| e.into_inner());
        self.fingerprints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
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
            match self.open_cached(&scope, &snapshot.owner_id) {
                Ok(p) => opened.push(p),
                Err(e) => tracing::warn!(
                    partition = %snapshot.partition_key,
                    "memory: partition skipped during rebuild: {e}"
                ),
            }
        }
        for p in &opened {
            // A partition that cannot be read now is left to the first read
            // of it, whose stat scan indexes a partition the index lacks.
            let loaded = match p.load() {
                Ok(loaded) => loaded,
                Err(e) => {
                    tracing::warn!(
                        partition = p.partition_key(),
                        "memory: partition skipped during rebuild: {e}"
                    );
                    continue;
                }
            };
            for batch in Self::records_from(p, &loaded).chunks(index::REBUILD_BATCH) {
                self.index.upsert_many(batch)?;
            }
            self.index
                .stamp_scan(p.partition_key(), loaded.scan.len())?;
        }
        self.index.finish_rebuild()?;
        // Re-read from the rows on each partition's next pass.
        self.fingerprints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        Ok(())
    }

    /// The notes of `partitions` as the index would hold them, read from
    /// their files (design §5, a rebuilding index): the pinned ones all,
    /// the others at most [`FILE_SCAN_MAX_NOTES`], most recently updated
    /// first.
    fn file_scan(&self, partitions: &[&Partition]) -> Result<FileScan, MemoryError> {
        let loaded: Vec<(&Partition, LoadedPartition)> = partitions
            .iter()
            .map(|p| Ok::<_, MemoryError>((*p, p.load()?)))
            .collect::<Result<_, _>>()?;
        let mut candidates: Vec<(&Partition, &NoteView, FileStat)> = Vec::new();
        let mut pinned = Vec::new();
        for (p, l) in &loaded {
            for (view, stat) in Self::indexable(l) {
                if view.note.pinned {
                    pinned.push(Self::record_for(p, view, stat));
                }
                candidates.push((*p, view, stat));
            }
        }
        candidates.sort_by(|a, b| {
            b.1.note
                .updated_at
                .cmp(&a.1.note.updated_at)
                .then_with(|| a.1.note.id.cmp(&b.1.note.id))
        });
        let truncated = candidates.len() > FILE_SCAN_MAX_NOTES;
        candidates.truncate(FILE_SCAN_MAX_NOTES);
        let records = candidates
            .into_iter()
            .map(|(p, view, stat)| Self::record_for(p, view, stat))
            .collect();
        Ok(FileScan {
            records,
            pinned,
            truncated,
        })
    }

    /// The index's state for an answer: `rebuilding` while a rebuild is
    /// owed or running (without asking the index), else what it records.
    fn current_index_state(&self) -> Result<IndexState, MemoryError> {
        if !self.index_serves() {
            return Ok(IndexState::Rebuilding);
        }
        Ok(self.index.state()?)
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
        let full_body = Self::body_to_read(&view, input.id, input.section.as_deref())?;
        let (body, truncated) = cut_bytes(&full_body, READ_MAX_BYTES);
        let out = Self::read_output(p, &view, body, truncated);
        self.record_reads(ctx, std::slice::from_ref(&out));
        Ok(out)
    }

    /// `memory_read` with its ids (design 09 §13.1, MD-14): every note of
    /// 1–[`READ_MAX_IDS`] distinct ids in one call, in the order asked, each
    /// with its revision and provenance, the bodies sharing the one
    /// [`READ_MAX_BYTES`] budget (a cut note says `truncated`).
    ///
    /// Each id is looked up in the owner's own partitions, as a single read
    /// is: one no partition has — another owner's note as much as one that
    /// never existed — answers [`ReadManyItem::NotFound`] in its place and
    /// changes nothing about the others. `expected_revision` and `section`
    /// go with a single id.
    pub fn read_many(
        &self,
        ctx: &MemoryAccessContext,
        input: ReadManyInput,
    ) -> Result<ReadManyOutput, MemoryError> {
        Self::require(ctx.capabilities.read, "memory_read")?;
        if input.ids.is_empty() || input.ids.len() > READ_MAX_IDS {
            return Err(MemoryError::invalid(format!(
                "ids takes 1 to {READ_MAX_IDS} note ids, got {}",
                input.ids.len()
            )));
        }
        let mut seen = HashSet::new();
        if let Some(twice) = input.ids.iter().find(|id| !seen.insert(**id)) {
            return Err(MemoryError::invalid(format!(
                "ids names note {twice} twice; name each note once"
            )));
        }
        if input.ids.len() > 1 && (input.expected_revision.is_some() || input.section.is_some()) {
            return Err(MemoryError::invalid(
                "expected_revision and section go with a single id",
            ));
        }
        let resolved = self.resolve(ctx)?;
        let loaded: Vec<(&Partition, LoadedPartition)> = resolved
            .all()
            .into_iter()
            .map(|p| Ok::<_, MemoryError>((p, p.load()?)))
            .collect::<Result<_, _>>()?;
        // Every id resolved first, so the budget is shared by what is read.
        let mut found: Vec<Result<(&Partition, &NoteView, String), ReadManyItem>> = Vec::new();
        for id in &input.ids {
            let hit = loaded.iter().find_map(|(p, l)| l.get(id).map(|v| (*p, v)));
            let Some((p, view)) = hit else {
                found.push(Err(ReadManyItem::NotFound(*id)));
                continue;
            };
            if let Some(expected) = &input.expected_revision
                && *expected != view.revision
            {
                found.push(Err(ReadManyItem::RevisionConflict {
                    id: *id,
                    current_revision: view.revision.clone(),
                }));
                continue;
            }
            let body = Self::body_to_read(view, *id, input.section.as_deref())?;
            found.push(Ok((p, view, body)));
        }
        let sizes: Vec<usize> = found
            .iter()
            .filter_map(|f| f.as_ref().ok().map(|(_, _, body)| body.len()))
            .collect();
        let mut shares = split_evenly(&sizes, READ_MAX_BYTES).into_iter();
        let mut items = Vec::with_capacity(found.len());
        let mut read = Vec::new();
        for entry in found {
            match entry {
                Ok((p, view, body)) => {
                    let share = shares.next().unwrap_or(0);
                    let (body, truncated) = cut_bytes(&body, share);
                    let out = Self::read_output(p, view, body, truncated);
                    read.push(out.clone());
                    items.push(ReadManyItem::Read(Box::new(out)));
                }
                Err(item) => items.push(item),
            }
        }
        self.record_reads(ctx, &read);
        Ok(ReadManyOutput {
            items,
            budget_bytes: READ_MAX_BYTES,
        })
    }

    /// The text a read serves: the whole body, or the section `section`
    /// heads (an absent one is `invalid_input`).
    fn body_to_read(
        view: &NoteView,
        id: NoteId,
        section: Option<&str>,
    ) -> Result<String, MemoryError> {
        match section {
            Some(heading) => section_of(&view.note.body, heading).ok_or_else(|| {
                MemoryError::invalid(format!("note {id} has no section headed `{heading}`"))
            }),
            None => Ok(view.note.body.clone()),
        }
    }

    fn read_output(p: &Partition, view: &NoteView, body: String, truncated: bool) -> ReadOutput {
        let note = &view.note;
        ReadOutput {
            id: note.id,
            revision: view.revision.clone(),
            title: note.title.clone(),
            kind: note.kind,
            scope: scope_of(p),
            status: note.status,
            effective_status: view.derived.effective_status,
            origin: view.derived.origin,
            frontmatter: management::frontmatter_json(note),
            body,
            truncated,
            path: format!("{}/{}", files::NOTES_DIR, view.file_name),
            sources: note.sources.clone(),
            supersedes: note.supersedes.clone(),
            superseded_by: view.derived.superseded_by.clone(),
            freshness: if view.unverified {
                Freshness::Unverified
            } else {
                Freshness::Verified
            },
            conflict: view.derived.conflict,
            source_forgotten: view.derived.source_forgotten,
        }
    }

    /// A read is a delivery on the `ModelTool` surface (design §7a,
    /// `memory_used` reason `tool`).
    fn record_reads(&self, ctx: &MemoryAccessContext, reads: &[ReadOutput]) {
        if ctx.surface != Surface::ModelTool {
            return;
        }
        let used: Vec<MemoryUsed> = reads
            .iter()
            .map(|r| MemoryUsed {
                id: r.id,
                revision: r.revision.clone(),
                reason: UsedReason::Tool,
            })
            .collect();
        self.record_used(ctx, &used);
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

    /// The notes whose id starts with `prefix` (design 09 §12.1, `/forget
    /// <id-or-prefix>`): every note of the context's partitions — any status,
    /// superseded and candidates included, as `openalpaca memory` resolves a
    /// prefix — newest update first, each with its current revision. The
    /// prefix is at least [`ID_PREFIX_MIN_HEX`] hex characters; case and
    /// hyphens are ignored on both sides, so `55E811B63220` finds
    /// `55e811b6-3220-…`. Records nothing: a lookup is not a recall.
    pub fn find_by_id_prefix(
        &self,
        ctx: &MemoryAccessContext,
        prefix: &str,
    ) -> Result<Vec<ListItem>, MemoryError> {
        Self::require(ctx.capabilities.read, "memory_read")?;
        let wanted: String = prefix.chars().filter(|c| *c != '-').collect();
        if !wanted.chars().all(|c| c.is_ascii_hexdigit()) || wanted.len() < ID_PREFIX_MIN_HEX {
            return Err(MemoryError::invalid(format!(
                "`{prefix}` is not a note id or a prefix of one (at least {ID_PREFIX_MIN_HEX} hex characters)"
            )));
        }
        let wanted = wanted.to_ascii_lowercase();
        let resolved = self.resolve(ctx)?;
        let mut items = Vec::new();
        for p in resolved.all() {
            let scope = scope_of(p);
            for view in p.load()?.views {
                let id: String = view.note.id.to_string().replace('-', "");
                if !id.starts_with(&wanted) {
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
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.to_string().cmp(&b.id.to_string()))
        });
        Ok(items)
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
        // Design 09 §13.3: the one active `extracted_other` note is a plain
        // `episode` — "it can supersede nothing" and carries no key.
        if input.provenance == Some(ExtractionProvenance::Summary)
            && (input.kind != Some(files::NoteKind::Episode)
                || input.fact_key.is_some()
                || input.supersedes.is_some()
                || input.replace_policy != ReplacePolicy::Direct)
        {
            return Err(MemoryError::invalid(
                "a conversation summary is a plain `episode`: no fact_key, no supersedes and no replace policy",
            ));
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
            (Surface::Extraction, Some(ExtractionProvenance::Summary)) => {
                (NoteStatus::Active, Some(Origin::ExtractedOther))
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
        let partition = self.open_partition(&store_scope, &ctx.owner_id)?;
        let (created, view) = partition.create_viewed(new, actor)?;
        self.reindex_after_write(&partition, &view);
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
            let fresh = self.open_partition(p.scope(), &ctx.owner_id)?;
            let (found, view) = fresh.reconcile_viewed()?;
            scanned += view.scan.len();
            for change in found {
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
            self.reindex_view(&fresh, &view)?;
        }
        Ok(SyncReport {
            scanned,
            changes,
            index_state: self.current_index_state()?,
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

/// Splits `total` bytes over bodies of these sizes (design 09 §13.1, a batch
/// read): evenly, and a body shorter than its equal share leaves the rest
/// to the others — no body gets more than an equal share of what the
/// shorter ones left, and the shares never sum past `total`. In the order
/// given.
pub(super) fn split_evenly(sizes: &[usize], total: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| sizes[i]);
    let mut shares = vec![0; sizes.len()];
    let mut left = total;
    for (served, &i) in order.iter().enumerate() {
        let equal = left / (sizes.len() - served);
        shares[i] = sizes[i].min(equal);
        left -= shares[i];
    }
    shares
}

/// A note's approximate size in tokens (design 09 §13.1): its body's bytes
/// ÷ 4, rounded up.
pub(super) fn approx_tokens(bytes: usize) -> usize {
    bytes.div_ceil(4)
}

/// Which end of a calendar day a bare date stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayEdge {
    /// Its first second: a `date_from`, a timeline's `date`.
    Start,
    /// Its last second: a `date_to`, so an inclusive bound keeps that day.
    End,
}

/// A date filter's bound or a timeline's instant (design 09 §13.1): an RFC
/// 3339 instant, or a calendar date `YYYY-MM-DD` taken in UTC at the edge
/// `edge` names. `None` for anything else.
pub fn parse_date_bound(value: &str, edge: DayEdge) -> Option<DateTime<Utc>> {
    let value = value.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(value) {
        return Some(t.with_timezone(&Utc));
    }
    let day = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    let time = match edge {
        DayEdge::Start => chrono::NaiveTime::MIN,
        DayEdge::End => chrono::NaiveTime::from_hms_opt(23, 59, 59)?,
    };
    Some(day.and_time(time).and_utc())
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
