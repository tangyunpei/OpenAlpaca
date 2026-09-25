//! A memory partition on disk: `<store>/memory/owners/<owner-key>/` with its
//! `notes/`, `.history/` and `MEMORY.md`, bound to one control snapshot under
//! `<home-store>/state/memory-control/<partition-key>.json` (design §2).
//!
//! **Opening never creates anything** (a read never creates the project
//! store, design §3 rule 1); the first write lays the partition out lazily
//! through `store::content_dir` — private directories, the project
//! `.gitignore`, an initial snapshot — in one place, [`Partition::ensure_laid_out`].
//!
//! **Every mutation touches at most one note file** (design §6): the steps of
//! the mutation table are [`Partition::commit_write`] and [`Partition::forget`],
//! the recovery column is [`Partition::recover`], run at every open of a
//! partition whose snapshot is valid. In tests a [`CrashPoint`] stops a
//! mutation after a named step with the disk exactly as it was.

use super::authority::{Conflict, Derivation, DerivedNote, EffectiveStatus, NoteInput, derive};
use super::cache::{FileEntry, NoteParseCache, ParsedNote, StatKey};
use super::fsx::{
    ComponentState, canonical_string_lenient, create_private_dir, fsync_dir, read_regular_nofollow,
    real_directory_state, remove_dir_all_if_exists, remove_file_if_exists, write_atomic_private,
};
use super::note::{NewNote, Note, NoteId, NotePatch, NoteStatus, ReplacePolicy, to_second};
use super::snapshot::{
    ControlSnapshot, NoteRecord, Origin, Pending, SnapshotError, SnapshotScope, Tombstone,
    save_generation,
};
use super::{
    CONTROL_DIR, HISTORY_DIR, HISTORY_KEEP, MemoryFilesError, NAVIGATION_FILE, NOTES_DIR,
    OWNERS_DIR, PROJECT_GITIGNORE_TEXT, home_partition_key, is_owner_key, is_partition_key,
    is_revision, owner_key, project_partition_key,
};
use crate::store::{
    ContentKind, STORE_DIR_NAME, StoreScope, content_dir, home_root, state_dir, store_root,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

thread_local! {
    /// How many whole-partition parses this thread has made — the cost
    /// design 09 §4.5 keeps off the reply path. Read by tests only.
    static WHOLE_PARSES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// How many `notes/` stat scans this thread has made (design 09 §6.3).
    /// Read by tests only.
    static SCANS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// How many note files this thread has read from disk (design 09 §6.3).
    /// Read by tests only.
    static FILE_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Tests: which snapshot save of this thread fails, counting the next
    /// as 1; 0 fails none ([`fail_snapshot_save`]).
    #[cfg(test)]
    static FAILING_SAVE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// Tests: whether the stat of the next note file this thread writes
    /// fails ([`fail_written_stat`]).
    #[cfg(test)]
    static FAILING_STAT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Tests: the `n`th snapshot save this thread makes from now (1: the next)
/// fails before anything is written, as a disk that refused the file would.
#[cfg(test)]
pub(super) fn fail_snapshot_save(n: u32) {
    FAILING_SAVE.with(|c| c.set(n));
}

/// Tests: the stat of the next note file this thread writes fails once it
/// is in place, as it would for a file removed or made unreadable a moment
/// after its rename.
#[cfg(test)]
pub(super) fn fail_written_stat() {
    FAILING_STAT.with(|f| f.set(true));
}

/// The whole-partition reads this thread has made so far — every note
/// file listed and served, parsed now or taken from the parse cache (design
/// 09 §6.3; [`note_file_reads_on_this_thread`] counts the files actually
/// read). A test-only counter: a caller on the reply path proves it reads
/// no partition by reading it before and after (design 09 §4.5,
/// `pending_suggestions_parses_no_partition`). One thread-local increment
/// per read, so a test is not disturbed by others running in parallel.
#[doc(hidden)]
pub fn whole_parses_on_this_thread() -> u64 {
    WHOLE_PARSES.with(|n| n.get())
}

/// The `notes/` stat scans ([`Partition::scan`]) this thread has made so
/// far — a test-only counter like [`whole_parses_on_this_thread`].
#[doc(hidden)]
pub fn scans_on_this_thread() -> u64 {
    SCANS.with(|n| n.get())
}

/// The note files this thread has read from disk so far — a whole
/// partition read counts every file the parse cache did not vouch for, and
/// a mutation's re-read of the file it checks counts one (design 09 §6.3).
/// A test-only counter like [`whole_parses_on_this_thread`].
#[doc(hidden)]
pub fn note_file_reads_on_this_thread() -> u64 {
    FILE_READS.with(|n| n.get())
}

/// Reads one note file, counted.
fn read_note_file(path: &Path) -> Result<Vec<u8>> {
    FILE_READS.with(|n| n.set(n.get() + 1));
    read_regular_nofollow(path)
}

// ============================================================================
// Who is writing
// ============================================================================

/// The surface a mutation comes from, mapped to the origin it records
/// (design §4, §7a): a user-directed surface records `user`; a model tool
/// call records `extracted_user`; the extractor records what it was told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    UserSurface,
    ModelTool,
    Extraction { origin: Origin },
}

impl Actor {
    pub fn origin(self) -> Origin {
        match self {
            Actor::UserSurface => Origin::User,
            Actor::ModelTool => Origin::ExtractedUser,
            Actor::Extraction { origin } => origin,
        }
    }

    /// Exactly the API, the CLI and the GUI (design §4).
    pub fn is_user_directed(self) -> bool {
        matches!(self, Actor::UserSurface)
    }
}

// ============================================================================
// Results
// ============================================================================

/// Whether a `create` wrote a file or answered with an exact duplicate
/// (design §6 *Deduplication*, §7b P8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dedup {
    Created,
    Existing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub id: NoteId,
    pub revision: String,
    pub origin: Origin,
    pub dedup: Dedup,
    /// The ids this note's frontmatter names (the `memory_store` result's
    /// `superseded`).
    pub superseded: Vec<NoteId>,
    /// The note is a **suggestion** (design 09 §3.1): a candidate that names
    /// a note in `supersedes` and retires nothing until the user confirms it
    /// — what [`ReplacePolicy::SuggestUnlessLearned`] writes for a target the
    /// user saved or edited (S2), and an inferred candidate with a target.
    pub suggestion: bool,
}

/// `memory_update.action` (design §7a).
#[derive(Debug, Clone)]
pub enum UpdateAction {
    Edit(NotePatch),
    Archive,
    /// `target_revision` binds the note a suggestion replaces (design 09
    /// §3.2, S5): required when the candidate carries `supersedes`, and a
    /// target that moved or went is `revision_conflict` or `not_found`, the
    /// candidate left as it is. Ignored for a candidate that names nothing.
    Confirm {
        target_revision: Option<String>,
    },
}

/// What a forget leaves behind (design 09 §3.3, ruling LR12). Both remove
/// the one note and its `.history`, tombstone first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgetMode {
    /// "Forget what I said" (design §6, *Forget contract*): the tombstone
    /// keeps the note's `supersedes` — so a forgotten replacement keeps
    /// retiring what it replaced — and suppresses every source it cited.
    /// A candidate's tombstone carries no links (S3): it retired nothing
    /// while it lived, and must not start now.
    Forget,
    /// "Keep my note; take back what learning did": undo, a dismissed
    /// suggestion. The tombstone carries **no links and no suppressed
    /// source**, so what the note replaced comes back and every other note
    /// the same message taught stays learned and recallable.
    Withdraw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Updated {
    pub id: NoteId,
    pub revision: String,
    pub status: NoteStatus,
    /// `false` when the patch changed nothing and no write happened.
    pub written: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgetStatus {
    Forgotten,
    RevisionConflict { current_revision: String },
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgetOutcome {
    pub id: NoteId,
    pub status: ForgetStatus,
}

/// One `notes/*.md` entry as the stat scan sees it (design §5 *Freshness*).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanEntry {
    pub path: PathBuf,
    pub file_name: String,
    /// The id the file name starts with, when it has the canon's shape.
    pub id: Option<NoteId>,
    pub size: u64,
    pub mtime: Option<SystemTime>,
    /// `0` where the platform has none.
    pub inode: u64,
}

/// A file under `notes/` that is not a note: kept on disk, excluded, never
/// rewritten (design §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MalformedFile {
    pub path: PathBuf,
    pub reason: String,
}

/// One note as read from disk, with everything the derivation says about it.
#[derive(Debug, Clone)]
pub struct NoteView {
    pub note: Note,
    /// The hash of the bytes actually read.
    pub revision: String,
    pub path: PathBuf,
    pub file_name: String,
    pub derived: DerivedNote,
    /// `true` when the partition is quarantined (design §2): explicit reads
    /// still work, but nothing here is verified against a snapshot.
    pub unverified: bool,
}

/// A partition read whole: every note that could be served, and what could
/// not.
#[derive(Debug, Clone)]
pub struct LoadedPartition {
    pub health: PartitionHealth,
    pub views: Vec<NoteView>,
    pub malformed: Vec<MalformedFile>,
    /// Ids that two or more files claim; none of those files is served.
    pub duplicates: Vec<(NoteId, Vec<PathBuf>)>,
    pub conflicts: Vec<Conflict>,
    /// The stat scan this view was read from, sorted by file name — after a
    /// write, with the written file's own stat — so a caller that indexes
    /// the view needs no second scan (design 09 §6.3 quick win 3).
    pub scan: Vec<ScanEntry>,
}

impl LoadedPartition {
    pub fn get(&self, id: &NoteId) -> Option<&NoteView> {
        self.views.iter().find(|v| v.note.id == *id)
    }
}

/// What the snapshot says at open (design §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartitionHealth {
    /// No directory and no snapshot: nothing to read, and the first write
    /// creates it.
    Absent,
    Ready,
    /// The snapshot is missing over existing notes, or invalid. Only
    /// `doctor --repair` lifts it.
    Quarantined(String),
}

/// A reconciliation scan's finding (design §5 *Freshness*, `memory_changed`
/// with `change: external`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileChangeKind {
    Added,
    Modified,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileChange {
    pub id: NoteId,
    pub kind: ReconcileChangeKind,
    /// The file's revision now (`None` when removed).
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairReport {
    pub partition_key: String,
    /// `false` when the partition was not quarantined: its preconditions
    /// were checked and nothing was rebuilt.
    pub rebuilt: bool,
    pub notes_recorded: usize,
    pub malformed: Vec<MalformedFile>,
}

/// Where a mutation is stopped in tests (design §6 table rows and fixture 15's
/// `crash_at.after_step`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashPoint {
    /// After step 1: `pending` set.
    WriteAfter1,
    /// Inside step 2: the `.history` copy written and fsynced, the note not
    /// yet renamed into place.
    WriteAfter2a,
    /// After step 2: the note written, `.history` pruned.
    WriteAfter2,
    /// After step 3: `pending` cleared, the record written.
    WriteAfter3,
    /// After forget step 1: the tombstone appended.
    ForgetAfter1,
    /// After forget step 2: the files deleted.
    ForgetAfter2,
}

// ============================================================================
// The partition
// ============================================================================

/// A partition's files as one read found them: the stat scan, every note
/// that parsed (with the revision of its bytes), the malformed files and
/// the ids two files claim. A mutation patches it with what it changed on
/// disk instead of reading the partition again (design 09 §6.3 quick wins 2
/// and 3), so the view it returns is the one a fresh read would give.
#[derive(Debug, Clone, Default)]
struct Parsed {
    scan: Vec<ScanEntry>,
    notes: Vec<(ScanEntry, Arc<ParsedNote>)>,
    malformed: Vec<MalformedFile>,
    duplicates: Vec<(NoteId, Vec<PathBuf>)>,
}

impl Parsed {
    /// Re-derives the duplicate ids from the parsed notes.
    fn find_duplicates(&mut self) {
        let mut by_id: BTreeMap<NoteId, Vec<PathBuf>> = BTreeMap::new();
        for (entry, parsed) in &self.notes {
            by_id
                .entry(parsed.note.id)
                .or_default()
                .push(entry.path.clone());
        }
        self.duplicates = by_id
            .into_iter()
            .filter(|(_, paths)| paths.len() > 1)
            .collect();
    }

    /// Forgets every file at one of `paths`: gone from the disk.
    fn drop_paths(&mut self, paths: &HashSet<PathBuf>) {
        self.scan.retain(|e| !paths.contains(&e.path));
        self.notes.retain(|(e, _)| !paths.contains(&e.path));
        self.malformed.retain(|m| !paths.contains(&m.path));
        self.find_duplicates();
    }

    /// Records the file this process just wrote — `bytes` at `entry` —
    /// replacing whatever was at `replaced` (the file written over, and
    /// under a retitle the name it moved from). The bytes are classified
    /// exactly as a read of the file would classify them; the note, when
    /// they are one, is returned.
    fn put(
        &mut self,
        replaced: &HashSet<PathBuf>,
        entry: ScanEntry,
        bytes: &[u8],
    ) -> Option<Arc<ParsedNote>> {
        self.scan.retain(|e| !replaced.contains(&e.path));
        self.notes.retain(|(e, _)| !replaced.contains(&e.path));
        self.malformed.retain(|m| !replaced.contains(&m.path));
        let written = match classify(&entry, Ok(bytes.to_vec())) {
            Ok(parsed) => {
                self.notes.push((entry.clone(), Arc::clone(&parsed)));
                self.notes.sort_by(|a, b| a.0.file_name.cmp(&b.0.file_name));
                Some(parsed)
            }
            Err(file) => {
                self.malformed.push(file);
                self.malformed.sort_by(|a, b| a.path.cmp(&b.path));
                None
            }
        };
        self.scan.push(entry);
        self.scan.sort_by(|a, b| a.file_name.cmp(&b.file_name));
        self.find_duplicates();
        written
    }
}

/// One `notes/*.md` entry from its `symlink_metadata`.
fn scan_entry(path: PathBuf, file_name: String, meta: &fs::Metadata) -> ScanEntry {
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    };
    #[cfg(not(unix))]
    let inode = 0u64;
    ScanEntry {
        id: Note::id_from_file_name(&file_name),
        path,
        file_name,
        size: meta.len(),
        mtime: meta.modified().ok(),
        inode,
    }
}

/// What one scanned file is: a note with the revision of its bytes, or a
/// malformed file and why — unreadable, unparsable, or named for another
/// id than its frontmatter carries.
fn classify(
    entry: &ScanEntry,
    read: Result<Vec<u8>, String>,
) -> Result<Arc<ParsedNote>, MalformedFile> {
    let malformed = |reason: String| MalformedFile {
        path: entry.path.clone(),
        reason,
    };
    let bytes = read.map_err(malformed)?;
    let note = Note::parse(&bytes).map_err(|e| malformed(e.reason))?;
    if let Some(named) = entry.id
        && named != note.id
    {
        return Err(malformed(format!(
            "the file name says {named} but the frontmatter id is {}",
            note.id
        )));
    }
    let revision = super::sha256_hex(&bytes);
    Ok(Arc::new(ParsedNote { note, revision }))
}

/// What a forget has done so far: each target's outcome, the files it
/// deleted and the ids it forgot.
#[derive(Default)]
struct ForgetProgress {
    outcomes: Vec<ForgetOutcome>,
    removed: HashSet<PathBuf>,
    forgotten: HashSet<NoteId>,
    /// The save generation of the last snapshot save, for the cache.
    saved: Option<u64>,
}

struct State {
    /// `None` until the first write lays the partition out, or while
    /// quarantined. Shared with the parse cache until a mutation changes it
    /// (`Arc::make_mut`).
    snapshot: Option<Arc<ControlSnapshot>>,
    quarantine: Option<String>,
}

pub struct Partition {
    scope: StoreScope,
    scope_kind: SnapshotScope,
    owner_id: String,
    owner_key: String,
    partition_key: String,
    memory_dir: PathBuf,
    root: PathBuf,
    root_canonical: String,
    notes_dir: PathBuf,
    history_dir: PathBuf,
    navigation_path: PathBuf,
    snapshot_path: PathBuf,
    state: Mutex<State>,
    clock: Clock,
    /// The service's parse cache (design 09 §6.3), when it opened this
    /// partition; `None` reads every file every time.
    cache: Option<Arc<NoteParseCache>>,
    #[cfg(test)]
    crash: Mutex<Option<CrashPoint>>,
}

impl std::fmt::Debug for Partition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Partition")
            .field("partition_key", &self.partition_key)
            .field("root", &self.root)
            .finish()
    }
}

/// The snapshot file of a partition key. A pure path query: it creates
/// nothing, and a `partition_key` that is not one of the two shapes of design
/// §2 (see [`is_partition_key`]) is `InvalidInput`, so the result is always a
/// file directly under `state/memory-control/`.
pub fn snapshot_path_for(partition_key: &str) -> Result<PathBuf, MemoryFilesError> {
    if !is_partition_key(partition_key) {
        return Err(MemoryFilesError::InvalidInput(format!(
            "`{partition_key}` is not a partition key (home-<owner-key> or proj-<owner-key>-<dir-key>)"
        )));
    }
    Ok(home_root()?
        .join("state")
        .join(CONTROL_DIR)
        .join(format!("{partition_key}.json")))
}

fn now_seconds() -> DateTime<Utc> {
    to_second(Utc::now())
}

/// Whether an open may write: an ordinary open re-asserts the project
/// `.gitignore` and runs recovery; a read-only one does neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenMode {
    ReadWrite,
    ReadOnly,
}

impl Partition {
    /// Opens the owner's partition in `scope` without creating anything.
    ///
    /// Design §3 rule 1: every component on the way — `<project>/.openalpaca`,
    /// `memory`, `owners`, `owners/<owner-key>`, `notes`, `.history` — is
    /// checked with `symlink_metadata`; a link, a junction or a non-directory
    /// at any level refuses the partition with [`MemoryFilesError::PathRefused`]
    /// naming it. The home root itself is not checked (it is the store's,
    /// possibly a link by design); the components below it are.
    ///
    /// A project partition whose `memory/` exists gets its `.gitignore`
    /// re-asserted (MD-7). A valid snapshot runs recovery (design §6).
    pub fn open(scope: &StoreScope, owner_id: &str) -> Result<Self, MemoryFilesError> {
        Self::open_inner(
            scope,
            owner_id,
            owner_key(owner_id),
            OpenMode::ReadWrite,
            None,
        )
    }

    /// [`Partition::open`] through the service's parse cache (design 09
    /// §6.3): the snapshot and every unchanged note file are taken from
    /// `cache` instead of being read and parsed again, and this partition's
    /// own writes keep the cache current.
    pub fn open_cached(
        scope: &StoreScope,
        owner_id: &str,
        cache: &Arc<NoteParseCache>,
    ) -> Result<Self, MemoryFilesError> {
        Self::open_inner(
            scope,
            owner_id,
            owner_key(owner_id),
            OpenMode::ReadWrite,
            Some(Arc::clone(cache)),
        )
    }

    /// Opens a partition **by owner key**, for reading only: the path checks
    /// of [`Partition::open`], the snapshot loaded and the quarantine
    /// decided, but no project `.gitignore` re-asserted and no recovery run
    /// — the two writes an ordinary open makes. What `doctor` (design §6,
    /// *Management operations*: "it changes nothing") opens every partition
    /// with, another owner's included, whose id it does not know. A key that
    /// is not 32 lowercase hex characters is refused before it is joined to
    /// anything. Mutations through a partition opened this way are refused
    /// by the ordinary quarantine check where they would be anyway; nothing
    /// else distinguishes it, so a caller that opened read-only must not
    /// write through it.
    pub fn open_readonly(scope: &StoreScope, owner_key: &str) -> Result<Self, MemoryFilesError> {
        if !is_owner_key(owner_key) {
            return Err(MemoryFilesError::InvalidInput(format!(
                "`{owner_key}` is not an owner key (32 lowercase hex characters)"
            )));
        }
        Self::open_inner(scope, "", owner_key.to_string(), OpenMode::ReadOnly, None)
    }

    fn open_inner(
        scope: &StoreScope,
        owner_id: &str,
        key: String,
        mode: OpenMode,
        cache: Option<Arc<NoteParseCache>>,
    ) -> Result<Self, MemoryFilesError> {
        let store = store_root(scope)?;
        let memory_dir = store.join(ContentKind::Memory.dir_name());
        let owners = memory_dir.join(OWNERS_DIR);
        let root = owners.join(&key);
        let notes_dir = root.join(NOTES_DIR);
        let history_dir = root.join(HISTORY_DIR);

        let mut components: Vec<&Path> = Vec::new();
        if matches!(scope, StoreScope::Project(_)) {
            components.push(&store);
        }
        components.extend([
            memory_dir.as_path(),
            owners.as_path(),
            root.as_path(),
            notes_dir.as_path(),
            history_dir.as_path(),
        ]);
        let mut memory_exists = false;
        for component in components {
            match real_directory_state(component) {
                Ok(ComponentState::Directory) => {
                    if component == memory_dir {
                        memory_exists = true;
                    }
                }
                Ok(ComponentState::Absent) => {}
                Err((component, reason)) => {
                    return Err(MemoryFilesError::PathRefused { component, reason });
                }
            }
        }

        let root_canonical = canonical_string_lenient(&root)?;
        let (scope_kind, partition_key) = match scope {
            StoreScope::Home => (SnapshotScope::Home, home_partition_key(&key)),
            StoreScope::Project(_) => (
                SnapshotScope::Project,
                project_partition_key(&key, &root_canonical),
            ),
        };
        let snapshot_path = snapshot_path_for(&partition_key)?;

        let partition = Self {
            scope: scope.clone(),
            scope_kind,
            owner_id: owner_id.to_string(),
            owner_key: key,
            partition_key: partition_key.clone(),
            memory_dir,
            navigation_path: root.join(NAVIGATION_FILE),
            root,
            root_canonical,
            notes_dir,
            history_dir,
            snapshot_path,
            state: Mutex::new(State {
                snapshot: None,
                quarantine: None,
            }),
            clock: Arc::new(now_seconds),
            cache,
            #[cfg(test)]
            crash: Mutex::new(None),
        };

        if mode == OpenMode::ReadWrite && scope_kind == SnapshotScope::Project && memory_exists {
            partition.assert_gitignore()?;
        }

        let loaded = match partition.load_snapshot() {
            Ok(Some(snapshot)) => match snapshot.check_binding(
                &partition_key,
                &partition.owner_key,
                &partition.root_canonical,
            ) {
                Ok(()) => Ok(Some(snapshot)),
                Err(reason) => Err(format!("snapshot binding: {reason}")),
            },
            Ok(None) => {
                if partition.has_note_files()? {
                    Err(format!(
                        "no control snapshot at {} for a partition that holds notes",
                        partition.snapshot_path.display()
                    ))
                } else {
                    Ok(None)
                }
            }
            Err(SnapshotError::Invalid(reason)) => Err(reason),
            Err(SnapshotError::Io(e)) => return Err(MemoryFilesError::Io(e)),
        };
        {
            let mut state = partition.lock();
            match loaded {
                Ok(snapshot) => state.snapshot = snapshot,
                Err(reason) => state.quarantine = Some(reason),
            }
        }
        if mode == OpenMode::ReadWrite {
            partition.recover()?;
        }
        Ok(partition)
    }

    /// Replaces the clock (tests).
    pub fn with_clock(mut self, clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    #[cfg(test)]
    pub(super) fn set_crash_point(&self, point: Option<CrashPoint>) {
        *self.crash.lock().unwrap_or_else(|p| p.into_inner()) = point;
    }

    fn crash_check(&self, point: CrashPoint) -> Result<(), MemoryFilesError> {
        #[cfg(test)]
        {
            let mut armed = self.crash.lock().unwrap_or_else(|p| p.into_inner());
            if *armed == Some(point) {
                *armed = None;
                return Err(MemoryFilesError::Crashed(point));
            }
        }
        #[cfg(not(test))]
        {
            let _ = point;
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn now(&self) -> DateTime<Utc> {
        to_second((self.clock)())
    }

    // ------------------------------------------------------------------------
    // The control snapshot, through the parse cache (design 09 §6.3)
    // ------------------------------------------------------------------------

    /// The snapshot file, parsed — or, through a cache, the parse this
    /// process last read or saved, while the file's stat is unchanged and no
    /// save came since. A file read from disk is cached once its stat has
    /// settled (the racy window, as for notes).
    fn load_snapshot(&self) -> Result<Option<Arc<ControlSnapshot>>, SnapshotError> {
        let Some(cache) = &self.cache else {
            return ControlSnapshot::load(&self.snapshot_path).map(|s| s.map(Arc::new));
        };
        let generation = save_generation();
        let taken = SystemTime::now();
        let stat = StatKey::lstat(&self.snapshot_path);
        if let Some(stat) = stat
            && let Some(hit) = cache.snapshot(
                &self.partition_key,
                &self.notes_dir,
                &self.snapshot_path,
                stat,
                generation,
            )
        {
            return Ok(Some(hit));
        }
        let loaded = ControlSnapshot::load(&self.snapshot_path)?.map(Arc::new);
        if let (Some(snapshot), Some(stat)) = (&loaded, stat)
            && cache.settled(&stat, taken)
        {
            cache.put_snapshot(
                &self.partition_key,
                &self.notes_dir,
                &self.snapshot_path,
                stat,
                generation,
                Arc::clone(snapshot),
            );
        }
        Ok(loaded)
    }

    /// The snapshot a mutation changes: unshared from the cache first.
    fn snapshot_mut(state: &mut State) -> &mut ControlSnapshot {
        Arc::make_mut(state.snapshot.as_mut().expect("a laid-out partition"))
    }

    /// Saves `snapshot` to the partition's file. Returns the save generation
    /// it was written at when no other save of this process came in between
    /// — the only one [`Partition::remember_snapshot`] may record it under.
    fn save_snapshot(
        &self,
        snapshot: &mut Arc<ControlSnapshot>,
        now: DateTime<Utc>,
    ) -> Result<Option<u64>, MemoryFilesError> {
        #[cfg(test)]
        if FAILING_SAVE.with(|c| c.replace(c.get().saturating_sub(1)) == 1) {
            return Err(MemoryFilesError::Io(anyhow::anyhow!(
                "injected snapshot save failure"
            )));
        }
        let before = save_generation();
        Arc::make_mut(snapshot).save(&self.snapshot_path, now)?;
        Ok((save_generation() == before + 1).then_some(before + 1))
    }

    /// Records `state`'s snapshot in the cache as saved at `generation` —
    /// once, after a mutation's last save, so a mutation's own saves do not
    /// each unshare it again. Skipped when another save has come since.
    fn remember_snapshot(&self, state: &State, generation: Option<u64>) {
        let (Some(cache), Some(generation), Some(snapshot)) =
            (&self.cache, generation, &state.snapshot)
        else {
            return;
        };
        let Some(stat) = StatKey::lstat(&self.snapshot_path) else {
            return;
        };
        if save_generation() != generation {
            return;
        }
        cache.put_snapshot(
            &self.partition_key,
            &self.notes_dir,
            &self.snapshot_path,
            stat,
            generation,
            Arc::clone(snapshot),
        );
    }

    // ------------------------------------------------------------------------
    // Accessors
    // ------------------------------------------------------------------------

    pub fn scope(&self) -> &StoreScope {
        &self.scope
    }
    pub fn owner_id(&self) -> &str {
        &self.owner_id
    }
    pub fn owner_key(&self) -> &str {
        &self.owner_key
    }
    pub fn partition_key(&self) -> &str {
        &self.partition_key
    }
    /// `…/memory/owners/<owner-key>`.
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// The canonical root string the snapshot binds to.
    pub fn root_canonical(&self) -> &str {
        &self.root_canonical
    }
    pub fn notes_dir(&self) -> &Path {
        &self.notes_dir
    }
    pub fn history_dir(&self) -> &Path {
        &self.history_dir
    }
    pub fn snapshot_path(&self) -> &Path {
        &self.snapshot_path
    }
    /// `<store>/memory`, where a project's `.gitignore` lives.
    pub fn memory_dir(&self) -> &Path {
        &self.memory_dir
    }

    pub fn health(&self) -> PartitionHealth {
        let state = self.lock();
        if let Some(reason) = &state.quarantine {
            PartitionHealth::Quarantined(reason.clone())
        } else if state.snapshot.is_some() {
            PartitionHealth::Ready
        } else {
            PartitionHealth::Absent
        }
    }

    /// A copy of the control snapshot, when the partition has one.
    pub fn snapshot(&self) -> Option<ControlSnapshot> {
        self.lock().snapshot.as_deref().cloned()
    }

    // ------------------------------------------------------------------------
    // Layout
    // ------------------------------------------------------------------------

    fn has_note_files(&self) -> Result<bool> {
        Ok(!self.scan()?.is_empty())
    }

    /// MD-7: the project partition's `.gitignore` is store-owned — rewritten
    /// atomically whenever its bytes are not `*\n`, so a cloned repository's
    /// force-added `!*` cannot stage the owner's notes.
    fn assert_gitignore(&self) -> Result<(), MemoryFilesError> {
        if self.scope_kind != SnapshotScope::Project {
            return Ok(());
        }
        let path = self.memory_dir.join(".gitignore");
        let current = match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_file() => read_regular_nofollow(&path).ok(),
            Ok(_) => None, // a link or a directory where our file belongs
            Err(_) => None,
        };
        if current.as_deref() != Some(PROJECT_GITIGNORE_TEXT.as_bytes()) {
            if let Ok(meta) = fs::symlink_metadata(&path)
                && meta.file_type().is_dir()
            {
                remove_dir_all_if_exists(&path)?;
            }
            write_atomic_private(&path, PROJECT_GITIGNORE_TEXT.as_bytes())?;
        }
        Ok(())
    }

    /// Lays the partition out on the first write: the store through
    /// `store::content_dir` (which seeds a project's `.openalpaca/`), `memory/`
    /// and the owner directories private (0700), the project `.gitignore`,
    /// and an initial empty snapshot — written last, so an interrupted layout
    /// is an empty directory the next open initializes again, never a
    /// snapshot over notes it did not record.
    fn ensure_laid_out(&self, state: &mut State) -> Result<(), MemoryFilesError> {
        if let Some(reason) = &state.quarantine {
            return Err(MemoryFilesError::Quarantined {
                partition_key: self.partition_key.clone(),
                reason: reason.clone(),
            });
        }
        if state.snapshot.is_some() {
            return Ok(());
        }
        // Re-check the path right before creating anything on it.
        for component in [
            self.memory_dir.as_path(),
            self.memory_dir.join(OWNERS_DIR).as_path(),
            self.root.as_path(),
            self.notes_dir.as_path(),
            self.history_dir.as_path(),
        ] {
            if let Err((component, reason)) = real_directory_state(component) {
                return Err(MemoryFilesError::PathRefused { component, reason });
            }
        }
        let memory = content_dir(&self.scope, ContentKind::Memory)?;
        create_private_dir(&memory)?;
        self.assert_gitignore()?;
        create_private_dir(&self.memory_dir.join(OWNERS_DIR))?;
        create_private_dir(&self.root)?;
        create_private_dir(&self.notes_dir)?;
        create_private_dir(&self.history_dir)?;
        let mut snapshot = Arc::new(ControlSnapshot::new(
            &self.partition_key,
            self.scope_kind,
            &self.owner_key,
            &self.owner_id,
            &self.root_canonical,
            self.now(),
        ));
        // `state/` itself is the store's and is created 0700 by the store,
        // never by the snapshot's `create_dir_all` with the umask default.
        state_dir()?;
        let saved = self.save_snapshot(&mut snapshot, self.now())?;
        state.snapshot = Some(snapshot);
        self.remember_snapshot(state, saved);
        self.write_navigation(&[]);
        Ok(())
    }

    /// Creates the partition now, with an empty snapshot (what a loader does;
    /// a service never needs it — the first write does the same).
    pub fn initialize(&self) -> Result<(), MemoryFilesError> {
        let mut state = self.lock();
        self.ensure_laid_out(&mut state)
    }

    // ------------------------------------------------------------------------
    // Scanning and loading
    // ------------------------------------------------------------------------

    /// Lists `notes/*.md`: regular files only — no hidden or temp names, no
    /// links, no directories — with the facts a stat scan compares.
    pub fn scan(&self) -> Result<Vec<ScanEntry>> {
        Ok(self.scan_keyed()?.into_iter().map(|(e, _)| e).collect())
    }

    /// [`Partition::scan`], each entry with the stat identity the parse
    /// cache compares — from the same `lstat`.
    fn scan_keyed(&self) -> Result<Vec<(ScanEntry, StatKey)>> {
        SCANS.with(|n| n.set(n.get() + 1));
        let entries = match fs::read_dir(&self.notes_dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to list {}", self.notes_dir.display()));
            }
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry =
                entry.with_context(|| format!("Failed to list {}", self.notes_dir.display()))?;
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if file_name.starts_with('.') || !file_name.ends_with(".md") {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.file_type().is_file() {
                continue;
            }
            out.push((scan_entry(path, file_name, &meta), StatKey::of(&meta)));
        }
        out.sort_by(|a, b| a.0.file_name.cmp(&b.0.file_name));
        Ok(out)
    }

    /// The scan entry of one file this process just wrote, and its stat
    /// identity.
    fn scan_one(&self, path: &Path) -> Result<(ScanEntry, StatKey)> {
        #[cfg(test)]
        if FAILING_STAT.with(|f| f.replace(false)) {
            anyhow::bail!("injected stat failure");
        }
        let meta = fs::symlink_metadata(path)
            .with_context(|| format!("Failed to stat {}", path.display()))?;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok((
            scan_entry(path.to_path_buf(), file_name, &meta),
            StatKey::of(&meta),
        ))
    }

    /// Scans and parses every file: the parsed notes (one per file), the
    /// malformed files and the duplicate ids. Through the parse cache
    /// (design 09 §6.3), a file whose stat identity is the cached one is not
    /// read — or, while that stat is within the racy window, is read and
    /// re-hashed but not parsed again — and what this read found replaces
    /// the partition's entries when it differs from them.
    fn parse_all(&self) -> Result<Parsed> {
        WHOLE_PARSES.with(|n| n.set(n.get() + 1));
        let taken = SystemTime::now();
        let keyed = self.scan_keyed()?;
        let cache = self.cache.as_deref();
        let cached = cache.and_then(|c| c.files(&self.partition_key, &self.notes_dir));
        let mut scan = Vec::with_capacity(keyed.len());
        let mut parsed = Vec::with_capacity(keyed.len());
        let mut malformed = Vec::new();
        // Beside each parsed note, the key and trust the cache would hold it
        // under; `changed` and `kept` say whether that differs from `cached`.
        let mut keys: Vec<Option<(StatKey, bool)>> = Vec::with_capacity(keyed.len());
        let mut changed = false;
        let mut kept = 0usize;
        for (entry, scanned) in keyed {
            let key = cache.and_then(|c| c.key(&entry.path, Some(scanned)));
            let hit = match (&cached, key) {
                (Some(files), Some(key)) => files.get(&entry.file_name).filter(|e| e.stat == key),
                _ => None,
            };
            let (found, trusted) = match hit {
                Some(hit) if hit.trusted => {
                    kept += 1;
                    (Ok(Arc::clone(&hit.parsed)), true)
                }
                _ => {
                    let read = read_note_file(&entry.path);
                    let settled = match (cache, key) {
                        (Some(c), Some(key)) => c.settled(&key, taken),
                        _ => false,
                    };
                    // A racy entry whose bytes still hash to it is reused,
                    // not parsed again.
                    let reuse = match (&read, hit) {
                        (Ok(bytes), Some(hit))
                            if super::sha256_hex(bytes) == hit.parsed.revision =>
                        {
                            Some(Arc::clone(&hit.parsed))
                        }
                        _ => None,
                    };
                    let unchanged = reuse.is_some() && !settled;
                    let found = match reuse {
                        Some(parsed) => Ok(parsed),
                        None => classify(&entry, read.map_err(|e| format!("{e:#}"))),
                    };
                    if unchanged {
                        kept += 1;
                    } else if found.is_ok() {
                        // New, promoted or replaced. (A file that is no note
                        // is never cached; its old entry, if any, is counted
                        // gone below.)
                        changed = true;
                    }
                    (found, settled)
                }
            };
            match found {
                Ok(note) => {
                    parsed.push((entry.clone(), note));
                    keys.push(key.map(|key| (key, trusted)));
                }
                Err(file) => malformed.push(file),
            }
            scan.push(entry);
        }
        if let Some(cache) = cache {
            let vanished = cached.as_ref().is_some_and(|files| files.len() != kept);
            if changed || vanished || cached.is_none() {
                self.store_parsed(cache, &parsed, &keys);
            }
        }
        let mut out = Parsed {
            scan,
            notes: parsed,
            malformed,
            duplicates: Vec::new(),
        };
        out.find_duplicates();
        Ok(out)
    }

    /// Replaces the partition's cache entries with what a read found — or
    /// leaves the partition uncached when it would not fit under the cap,
    /// which is decided before any entry is built.
    fn store_parsed(
        &self,
        cache: &NoteParseCache,
        parsed: &[(ScanEntry, Arc<ParsedNote>)],
        keys: &[Option<(StatKey, bool)>],
    ) {
        let entries: Vec<(&str, FileEntry)> = parsed
            .iter()
            .zip(keys)
            .filter_map(|((entry, note), key)| {
                let (stat, trusted) = (*key)?;
                Some((
                    entry.file_name.as_str(),
                    FileEntry::new(&entry.file_name, stat, Arc::clone(note), trusted),
                ))
            })
            .collect();
        let bytes: usize = entries.iter().map(|(_, e)| e.bytes).sum();
        let files = (bytes <= cache.capacity()).then(|| {
            entries
                .into_iter()
                .map(|(name, entry)| (name.to_string(), entry))
                .collect()
        });
        cache.store_files(&self.partition_key, &self.notes_dir, files);
    }

    fn load_with(&self, state: &State) -> Result<LoadedPartition, MemoryFilesError> {
        Ok(self.load_parsed(state)?.1)
    }

    /// Reads the partition once, keeping what was read so a mutation can
    /// patch it with its own change.
    fn load_parsed(&self, state: &State) -> Result<(Parsed, LoadedPartition), MemoryFilesError> {
        let parsed = self.parse_all()?;
        let loaded = self.derive_view(&parsed, state);
        Ok((parsed, loaded))
    }

    /// The invariant of the parse cache (design 09 §6.3): **a mutation
    /// re-reads and re-hashes each file it checks a revision against,
    /// before the check.** A view whose bytes are no longer the file's — a
    /// cached parse a same-stat edit slipped past, or an edit that raced
    /// this read — is dropped from the cache and the partition read again,
    /// so the check compares against the bytes on disk and a write never
    /// replaces an edit it did not see. A file that cannot be read any more
    /// counts as changed.
    fn recheck(
        &self,
        state: &State,
        parsed: &mut Parsed,
        loaded: &mut LoadedPartition,
        ids: &[NoteId],
    ) -> Result<(), MemoryFilesError> {
        let mut stale: Vec<String> = Vec::new();
        for id in ids {
            let Some(view) = loaded.get(id) else {
                continue;
            };
            let current = read_note_file(&view.path)
                .ok()
                .map(|bytes| super::sha256_hex(&bytes));
            if current.as_deref() != Some(view.revision.as_str()) {
                stale.push(view.file_name.clone());
            }
        }
        if stale.is_empty() {
            return Ok(());
        }
        if let Some(cache) = &self.cache {
            cache.remove_files(&self.partition_key, &self.notes_dir, &stale);
        }
        (*parsed, *loaded) = self.load_parsed(state)?;
        Ok(())
    }

    /// The derivation over what was read (design §6): every servable note
    /// with its derived facts, under the snapshot as it stands in `state`.
    fn derive_view(&self, parsed: &Parsed, state: &State) -> LoadedPartition {
        let dup_ids: HashSet<NoteId> = parsed.duplicates.iter().map(|(id, _)| *id).collect();
        let unique: Vec<&(ScanEntry, Arc<ParsedNote>)> = parsed
            .notes
            .iter()
            .filter(|(_, p)| !dup_ids.contains(&p.note.id))
            .collect();
        let empty;
        let (snapshot, unverified) = match (&state.snapshot, &state.quarantine) {
            (Some(s), None) => (&**s, false),
            _ => {
                empty = ControlSnapshot::new(
                    &self.partition_key,
                    self.scope_kind,
                    &self.owner_key,
                    &self.owner_id,
                    &self.root_canonical,
                    self.now(),
                );
                (&empty, true)
            }
        };
        let inputs: Vec<NoteInput<'_>> = unique
            .iter()
            .map(|(_, p)| NoteInput {
                note: &p.note,
                revision: &p.revision,
            })
            .collect();
        let Derivation {
            mut notes,
            conflicts,
        } = derive(&inputs, snapshot);
        let views = unique
            .into_iter()
            .map(|(entry, p)| NoteView {
                derived: notes.remove(&p.note.id).expect("derived for every input"),
                note: p.note.clone(),
                revision: p.revision.clone(),
                path: entry.path.clone(),
                file_name: entry.file_name.clone(),
                unverified,
            })
            .collect();
        let health = if let Some(reason) = &state.quarantine {
            PartitionHealth::Quarantined(reason.clone())
        } else if state.snapshot.is_some() {
            PartitionHealth::Ready
        } else {
            PartitionHealth::Absent
        };
        LoadedPartition {
            health,
            views,
            malformed: parsed.malformed.clone(),
            duplicates: parsed.duplicates.clone(),
            conflicts,
            scan: parsed.scan.clone(),
        }
    }

    /// Reads the whole partition: every servable note with its derived facts,
    /// plus the malformed files and duplicate ids. Works while quarantined
    /// (every view then reads `unverified`).
    pub fn load(&self) -> Result<LoadedPartition, MemoryFilesError> {
        let state = self.lock();
        self.load_with(&state)
    }

    /// One note by id; a duplicate id is not served.
    pub fn read(&self, id: &NoteId) -> Result<NoteView, MemoryFilesError> {
        let loaded = self.load()?;
        loaded
            .get(id)
            .cloned()
            .ok_or(MemoryFilesError::NotFound(*id))
    }

    // ------------------------------------------------------------------------
    // `.history`
    // ------------------------------------------------------------------------

    /// The service-written revisions kept for `id`, newest first.
    pub fn history(&self, id: &NoteId) -> Result<Vec<(String, Option<SystemTime>)>> {
        let dir = self.history_dir.join(id.to_string());
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("Failed to list {}", dir.display())),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(rev) = name.strip_suffix(".md") else {
                continue;
            };
            if !is_revision(rev) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if !meta.file_type().is_file() {
                continue;
            }
            out.push((rev.to_string(), meta.modified().ok()));
        }
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
        Ok(out)
    }

    /// The bytes of one kept revision.
    pub fn history_bytes(&self, id: &NoteId, revision: &str) -> Result<Option<Vec<u8>>> {
        if !is_revision(revision) {
            return Ok(None);
        }
        let path = self
            .history_dir
            .join(id.to_string())
            .join(format!("{revision}.md"));
        super::fsx::read_if_exists(&path)
    }

    fn history_file(&self, id: &NoteId, revision: &str) -> PathBuf {
        self.history_dir
            .join(id.to_string())
            .join(format!("{revision}.md"))
    }

    /// Keeps the newest [`HISTORY_KEEP`] revisions of `id`; `keep` is never
    /// pruned whatever its mtime says.
    fn prune_history(&self, id: &NoteId, keep: &str) -> Result<()> {
        let revisions = self.history(id)?;
        let mut others = 0usize;
        for (rev, _) in revisions {
            if rev == keep {
                continue;
            }
            if others < HISTORY_KEEP - 1 {
                others += 1;
                continue;
            }
            remove_file_if_exists(&self.history_file(id, &rev))?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------------
    // The mutation protocol (design §6, rows create / update / archive / confirm)
    // ------------------------------------------------------------------------

    /// The four steps of the table, each atomic. `from_rev` is `None` on
    /// create; `current_path` is the file to write onto for an existing note
    /// (a retitle renames it afterwards, so the recovery's hash test holds at
    /// every point). `parsed` is what the mutation read before deciding; it
    /// is patched with the file written, and the view derived from it — the
    /// partition as a fresh read would now find it — is returned beside the
    /// new revision and written to `MEMORY.md` (design 09 §6.3 quick win 3).
    fn commit_write(
        &self,
        state: &mut State,
        parsed: &mut Parsed,
        from_rev: Option<String>,
        note: &Note,
        origin: Origin,
        current_path: Option<&Path>,
    ) -> Result<(String, LoadedPartition), MemoryFilesError> {
        self.ensure_laid_out(state)?;
        let bytes = note.serialize();
        let to_rev = super::sha256_hex(bytes.as_bytes());

        // Step 1: the single pending field.
        Self::snapshot_mut(state).pending = Some(Pending {
            id: note.id,
            from_rev,
            to_rev: to_rev.clone(),
            origin,
        });
        self.save_snapshot(state.snapshot.as_mut().expect("laid out"), self.now())?;
        self.crash_check(CrashPoint::WriteAfter1)?;

        // Step 2a: the new bytes into `.history` before the note's rename.
        create_private_dir(&self.history_dir.join(note.id.to_string()))?;
        write_atomic_private(&self.history_file(&note.id, &to_rev), bytes.as_bytes())?;
        self.crash_check(CrashPoint::WriteAfter2a)?;

        // Step 2: the note (temp, fsync, rename), then the slug, then prune.
        let target = self.notes_dir.join(note.file_name());
        let write_to = current_path
            .map(Path::to_path_buf)
            .unwrap_or_else(|| target.clone());
        write_atomic_private(&write_to, bytes.as_bytes())?;
        if write_to != target {
            fs::rename(&write_to, &target).with_context(|| {
                format!(
                    "Failed to rename {} to {}",
                    write_to.display(),
                    target.display()
                )
            })?;
            fsync_dir(&self.notes_dir)?;
        }
        self.prune_history(&note.id, &to_rev)?;
        self.crash_check(CrashPoint::WriteAfter2)?;

        // Step 3: record the revision and origin, clear pending.
        let record = NoteRecord {
            revision: to_rev.clone(),
            origin,
            written_at: self.now(),
        };
        let snapshot = Self::snapshot_mut(state);
        snapshot.notes.insert(note.id, record);
        snapshot.pending = None;
        let saved = self.save_snapshot(state.snapshot.as_mut().expect("laid out"), self.now())?;
        self.remember_snapshot(state, saved);
        self.crash_check(CrashPoint::WriteAfter3)?;

        // Step 4: navigation (the index and the event are the caller's),
        // from what was read plus what was written — and the parse cache,
        // which takes the written file untrusted: the next read re-hashes
        // it (design 09 §6.3). The write has landed: a file that cannot be
        // stat'ed now costs the patched view and the cache entry, not the
        // write — the view is read again instead.
        let (entry, stat) = match self.scan_one(&target) {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!(
                    "Could not stat the note just written in {} ({}); reading the partition again",
                    self.partition_key,
                    e.root_cause()
                );
                let view;
                (*parsed, view) = self.load_parsed(state)?;
                self.write_navigation(&view.views);
                return Ok((to_rev, view));
            }
        };
        let file_name = entry.file_name.clone();
        let mut replaced_names = Vec::new();
        if write_to != target
            && let Some(name) = write_to.file_name()
        {
            replaced_names.push(name.to_string_lossy().into_owned());
        }
        let replaced: HashSet<PathBuf> = [write_to, target.clone()].into_iter().collect();
        let written = parsed.put(&replaced, entry, bytes.as_bytes());
        if let (Some(cache), Some(written)) = (&self.cache, written)
            && let Some(stat) = cache.key(&target, Some(stat))
        {
            cache.put_file(
                &self.partition_key,
                &self.notes_dir,
                &replaced_names,
                &file_name,
                FileEntry::new(&file_name, stat, written, false),
            );
        }
        let view = self.derive_view(parsed, state);
        self.write_navigation(&view.views);
        Ok((to_rev, view))
    }

    /// `memory_store` (design §7a): validates, checks a `supersedes` target's
    /// revision and authority, answers an exact duplicate with the existing
    /// id, else creates the note with a fresh UUID.
    pub fn create(&self, input: NewNote, actor: Actor) -> Result<Created, MemoryFilesError> {
        self.create_viewed(input, actor).map(|(created, _)| created)
    }

    /// [`Partition::create`], and the partition as it stands afterwards —
    /// what a fresh [`Partition::load`] would return, read once (design 09
    /// §6.3 quick win 3).
    pub fn create_viewed(
        &self,
        input: NewNote,
        actor: Actor,
    ) -> Result<(Created, LoadedPartition), MemoryFilesError> {
        let mut state = self.lock();
        self.refuse_if_quarantined(&state)?;
        let Some(kind) = input.kind else {
            return Err(MemoryFilesError::InvalidInput("kind is required".into()));
        };
        let mut status = match input.status.unwrap_or(NoteStatus::Active) {
            NoteStatus::Archived => {
                return Err(MemoryFilesError::InvalidInput(
                    "a note is not created archived".into(),
                ));
            }
            s => s,
        };
        let (mut parsed, mut loaded) = self.load_parsed(&state)?;
        if let Some((target, _)) = &input.supersedes {
            self.recheck(&state, &mut parsed, &mut loaded, &[*target])?;
        }
        let same_note = |v: &NoteView| {
            v.note.kind == kind
                && v.note.title == input.title
                && v.note.body == input.body
                && v.note.fact_key == input.fact_key
                && v.note.aliases == input.aliases
                && v.note.tags == input.tags
        };

        let mut supersedes = Vec::new();
        if let Some((target, expected)) = &input.supersedes {
            let view = loaded
                .get(target)
                .ok_or(MemoryFilesError::NotFound(*target))?;
            if view.revision != *expected {
                return Err(MemoryFilesError::RevisionConflict {
                    id: *target,
                    current_revision: view.revision.clone(),
                });
            }
            // S2 (design 09 §3.2, ruling LR9): learning replaces a learned
            // note outright, and suggests replacing any other — one the user
            // saved, edited by hand or imported. The one load above decides.
            if input.replace_policy == ReplacePolicy::SuggestUnlessLearned
                && view.derived.origin != Origin::ExtractedUser
            {
                status = NoteStatus::Candidate;
            }
            // S1: a surface the user did not direct may not *replace* a note
            // the user saved (G2) — but a candidate replaces nothing while it
            // stays one (design §6), and only a user-directed confirm, bound
            // to the target's revision (S5), makes it stop being one.
            if view.derived.origin == Origin::User
                && !actor.is_user_directed()
                && status != NoteStatus::Candidate
            {
                return Err(authority_conflict(*target, "replace"));
            }
            // S4: a restated change does not mint a second, identical
            // suggestion for the same target.
            if status == NoteStatus::Candidate
                && let Some(existing) = loaded.views.iter().find(|v| {
                    v.derived.effective_status == EffectiveStatus::Candidate
                        && v.note.supersedes == [*target]
                        && same_note(v)
                })
            {
                let created = Created {
                    id: existing.note.id,
                    revision: existing.revision.clone(),
                    origin: existing.derived.origin,
                    dedup: Dedup::Existing,
                    superseded: existing.note.supersedes.clone(),
                    suggestion: true,
                };
                return Ok((created, loaded));
            }
            supersedes.push(*target);
        } else if let Some(existing) = loaded
            .views
            .iter()
            .find(|v| v.derived.effective_status == EffectiveStatus::Active && same_note(v))
        {
            let created = Created {
                id: existing.note.id,
                revision: existing.revision.clone(),
                origin: existing.derived.origin,
                dedup: Dedup::Existing,
                superseded: Vec::new(),
                suggestion: false,
            };
            return Ok((created, loaded));
        }

        let now = self.now();
        let note = Note {
            id: NoteId::new(),
            title: input.title,
            kind,
            status,
            fact_key: input.fact_key,
            aliases: input.aliases,
            tags: input.tags,
            created_at: now,
            updated_at: now,
            observed_at: input.observed_at,
            valid_from: input.valid_from,
            valid_to: input.valid_to,
            review_after: None,
            pinned: false,
            sources: input.sources,
            supersedes: supersedes.clone(),
            unknown: Vec::new(),
            body: input.body,
        };
        note.validate_for_write()
            .map_err(MemoryFilesError::InvalidInput)?;
        let origin = actor.origin();
        let (revision, view) =
            self.commit_write(&mut state, &mut parsed, None, &note, origin, None)?;
        let created = Created {
            id: note.id,
            revision,
            origin,
            dedup: Dedup::Created,
            suggestion: status == NoteStatus::Candidate && !supersedes.is_empty(),
            superseded: supersedes,
        };
        Ok((created, view))
    }

    /// `memory_update` (design §7a): edit, archive or confirm, under the
    /// revision check and the G2 rules — `confirm` only from a user-directed
    /// surface; edit/archive of a `user` note only from one.
    pub fn update(
        &self,
        id: &NoteId,
        expected_revision: &str,
        action: UpdateAction,
        actor: Actor,
    ) -> Result<Updated, MemoryFilesError> {
        self.update_viewed(id, expected_revision, action, actor)
            .map(|(updated, _)| updated)
    }

    /// [`Partition::update`], and the partition as it stands afterwards —
    /// what a fresh [`Partition::load`] would return, read once (design 09
    /// §6.3 quick win 3).
    pub fn update_viewed(
        &self,
        id: &NoteId,
        expected_revision: &str,
        action: UpdateAction,
        actor: Actor,
    ) -> Result<(Updated, LoadedPartition), MemoryFilesError> {
        let mut state = self.lock();
        self.refuse_if_quarantined(&state)?;
        let (mut parsed, mut loaded) = self.load_parsed(&state)?;
        self.recheck(&state, &mut parsed, &mut loaded, &[*id])?;
        if matches!(action, UpdateAction::Confirm { .. }) {
            // The note a confirmed suggestion retires is bound by revision
            // too (S5): re-read it before that check as well.
            let targets = loaded
                .get(id)
                .map(|v| v.note.supersedes.clone())
                .unwrap_or_default();
            self.recheck(&state, &mut parsed, &mut loaded, &targets)?;
        }
        let view = loaded.get(id).ok_or(MemoryFilesError::NotFound(*id))?;
        if view.revision != expected_revision {
            return Err(MemoryFilesError::RevisionConflict {
                id: *id,
                current_revision: view.revision.clone(),
            });
        }
        let mut new = view.note.clone();
        // A suggestion (design 09 §3.1): a candidate that names a note it
        // would replace. It retires nothing while it stays a candidate, so
        // every way it could stop being one other than a bound confirm is
        // closed below (S9, S10).
        let suggestion = new.status == NoteStatus::Candidate && !new.supersedes.is_empty();
        let origin = match &action {
            UpdateAction::Confirm { target_revision } => {
                if !actor.is_user_directed() {
                    return Err(MemoryFilesError::ConfirmRefused {
                        id: *id,
                        message: format!(
                            "Confirming a remembered candidate is done by you, not by me: open the Memory view in the app, or run `openalpaca memory confirm {id}`."
                        ),
                    });
                }
                if new.status != NoteStatus::Candidate {
                    return Err(MemoryFilesError::InvalidInput(format!(
                        "note {id} is {}, not a candidate",
                        new.status
                    )));
                }
                if suggestion {
                    // S5: the confirm binds the note it retires, so a target
                    // edited after the suggestion is never retired unseen.
                    let [target] = new.supersedes.as_slice() else {
                        return Err(MemoryFilesError::InvalidInput(format!(
                            "note {id} names {} notes it would replace; a suggestion replaces one — edit its supersedes list before confirming it",
                            new.supersedes.len()
                        )));
                    };
                    let Some(target_revision) = target_revision else {
                        return Err(MemoryFilesError::InvalidInput(format!(
                            "note {id} would replace note {target}: confirming it needs target_revision, the revision of the note it replaces as you last saw it"
                        )));
                    };
                    let current = loaded
                        .get(target)
                        .ok_or(MemoryFilesError::NotFound(*target))?;
                    if current.revision != *target_revision {
                        return Err(MemoryFilesError::RevisionConflict {
                            id: *target,
                            current_revision: current.revision.clone(),
                        });
                    }
                    // A link retires its target without changing the
                    // target's file, so a revision that still matches does
                    // not mean the target is still the one in force.
                    if current.derived.effective_status != EffectiveStatus::Active {
                        return Err(MemoryFilesError::InvalidInput(format!(
                            "note {target}, which note {id} would replace, is no longer active (it is {}): there is nothing left for this suggestion to replace — undo it to dismiss it (`openalpaca memory undo {id}`, or Keep mine in the Memory view)",
                            current.derived.effective_status
                        )));
                    }
                }
                new.status = NoteStatus::Active;
                Origin::User
            }
            UpdateAction::Archive => {
                // S9: an archived former suggestion's link would count, and
                // retire its target with the archiving actor's authority.
                if suggestion {
                    return Err(MemoryFilesError::InvalidInput(format!(
                        "note {id} is a suggestion: a suggestion is replaced or kept, not archived — confirm it to replace the note it names, or undo it to keep yours"
                    )));
                }
                if view.derived.origin == Origin::User && !actor.is_user_directed() {
                    return Err(authority_conflict(*id, "archive"));
                }
                if new.status == NoteStatus::Archived {
                    let updated = Updated {
                        id: *id,
                        revision: view.revision.clone(),
                        status: new.status,
                        written: false,
                    };
                    return Ok((updated, loaded));
                }
                new.status = NoteStatus::Archived;
                actor.origin()
            }
            UpdateAction::Edit(patch) => {
                if view.derived.origin == Origin::User && !actor.is_user_directed() {
                    return Err(authority_conflict(*id, "edit"));
                }
                // S10: the text a confirmation shows is the extractor's, and
                // an edit here would also re-record an inferred suggestion as
                // the user's own words.
                if suggestion && !actor.is_user_directed() {
                    return Err(MemoryFilesError::AuthorityConflict {
                        id: *id,
                        message: format!(
                            "Note {id} is a suggestion waiting for you, so a tool call may not edit it: open the Memory view in the app, or run `openalpaca memory confirm {id}` or `openalpaca memory undo {id}`."
                        ),
                    });
                }
                patch.apply(&mut new);
                if new.serialize() == view.note.serialize() {
                    let updated = Updated {
                        id: *id,
                        revision: view.revision.clone(),
                        status: new.status,
                        written: false,
                    };
                    return Ok((updated, loaded));
                }
                actor.origin()
            }
        };
        new.updated_at = self.now();
        new.validate_for_write()
            .map_err(MemoryFilesError::InvalidInput)?;
        let path = view.path.clone();
        let from_rev = view.revision.clone();
        let status = new.status;
        let (revision, view) = self.commit_write(
            &mut state,
            &mut parsed,
            Some(from_rev),
            &new,
            origin,
            Some(&path),
        )?;
        let updated = Updated {
            id: *id,
            revision,
            status,
            written: true,
        };
        Ok((updated, view))
    }

    // ------------------------------------------------------------------------
    // Forget (design §6, tombstone first)
    // ------------------------------------------------------------------------

    /// `memory_forget` (design §7a): per target, a revision check, then the
    /// tombstone (with the note's origin, `supersedes` and source ids), then
    /// the file and its `.history`. Each target is its own single-note
    /// mutation. A target with no file answers `not_found` whatever revision
    /// was sent (§7b P12). [`ForgetMode::Forget`].
    pub fn forget(
        &self,
        targets: &[(NoteId, String)],
    ) -> Result<Vec<ForgetOutcome>, MemoryFilesError> {
        self.forget_as(targets, ForgetMode::Forget)
    }

    /// A forget or a withdrawal (design 09 §3.3): the same single-note
    /// mutations, tombstone first; `mode` decides what the tombstone keeps.
    pub fn forget_as(
        &self,
        targets: &[(NoteId, String)],
        mode: ForgetMode,
    ) -> Result<Vec<ForgetOutcome>, MemoryFilesError> {
        self.forget_viewed(targets, mode)
            .map(|(outcomes, _)| outcomes)
    }

    /// [`Partition::forget_as`], and the partition as it stands afterwards.
    /// The partition is read **once** for every target (design 09 §6.3 quick
    /// win 2): forgetting one note changes no other note's revision, origin
    /// or file, which is all a later target's checks read, and a target
    /// already forgotten in this call answers `not_found`, as a re-read
    /// would. `MEMORY.md` is rebuilt once, after the last target — and also
    /// when a target fails, for the ones forgotten before it.
    pub fn forget_viewed(
        &self,
        targets: &[(NoteId, String)],
        mode: ForgetMode,
    ) -> Result<(Vec<ForgetOutcome>, LoadedPartition), MemoryFilesError> {
        let mut state = self.lock();
        self.refuse_if_quarantined(&state)?;
        let (mut parsed, mut loaded) = self.load_parsed(&state)?;
        let ids: Vec<NoteId> = targets.iter().map(|(id, _)| *id).collect();
        self.recheck(&state, &mut parsed, &mut loaded, &ids)?;
        let mut done = ForgetProgress::default();
        let result = self.forget_each(&mut state, &parsed, &loaded, targets, mode, &mut done);
        self.remember_snapshot(&state, done.saved);
        if let Some(cache) = &self.cache
            && !done.removed.is_empty()
        {
            let names: Vec<String> = done
                .removed
                .iter()
                .filter_map(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .collect();
            cache.remove_files(&self.partition_key, &self.notes_dir, &names);
        }
        if done.forgotten.is_empty() {
            return result.map(|()| (done.outcomes, loaded));
        }
        parsed.drop_paths(&done.removed);
        let view = self.derive_view(&parsed, &state);
        self.write_navigation(&view.views);
        result.map(|()| (done.outcomes, view))
    }

    /// The per-target steps of [`Partition::forget_viewed`] over one read of
    /// the partition, recording in `done` what each did as it goes, so a
    /// failure part-way leaves an account of the targets before it.
    fn forget_each(
        &self,
        state: &mut State,
        parsed: &Parsed,
        loaded: &LoadedPartition,
        targets: &[(NoteId, String)],
        mode: ForgetMode,
        done: &mut ForgetProgress,
    ) -> Result<(), MemoryFilesError> {
        let ForgetProgress {
            outcomes,
            removed,
            forgotten,
            saved,
        } = done;
        for (id, expected) in targets {
            let view = if forgotten.contains(id) {
                None
            } else {
                loaded.get(id)
            };
            let Some(view) = view else {
                outcomes.push(ForgetOutcome {
                    id: *id,
                    status: ForgetStatus::NotFound,
                });
                continue;
            };
            if view.revision != *expected {
                outcomes.push(ForgetOutcome {
                    id: *id,
                    status: ForgetStatus::RevisionConflict {
                        current_revision: view.revision.clone(),
                    },
                });
                continue;
            }
            let (supersedes, suppressed_sources) = match mode {
                ForgetMode::Forget => (
                    // S3: a candidate retired nothing while it lived.
                    if view.note.status == NoteStatus::Candidate {
                        Vec::new()
                    } else {
                        view.note.supersedes.clone()
                    },
                    view.note
                        .sources
                        .iter()
                        .map(|s| s.suppression_key())
                        .collect(),
                ),
                ForgetMode::Withdraw => (Vec::new(), Vec::new()),
            };
            let tombstone = Tombstone {
                id: *id,
                origin: view.derived.origin,
                supersedes,
                suppressed_sources,
                forgotten_at: self.now(),
            };
            // Step 1: the tombstone, durable before anything is deleted.
            let snapshot = Self::snapshot_mut(state);
            snapshot.tombstones.push(tombstone);
            snapshot.notes.remove(id);
            // A save that fails leaves no earlier target's generation behind
            // to cache this target's unsaved tombstone under.
            *saved = None;
            *saved = self.save_snapshot(
                state.snapshot.as_mut().expect("not quarantined, has notes"),
                self.now(),
            )?;
            self.crash_check(CrashPoint::ForgetAfter1)?;

            // Step 2: the file (every file with that id) and its history.
            let files: Vec<&PathBuf> = std::iter::once(&view.path)
                .chain(
                    parsed
                        .scan
                        .iter()
                        .filter(|e| e.id == Some(*id))
                        .map(|e| &e.path),
                )
                .collect();
            for path in files {
                remove_file_if_exists(path)?;
                removed.insert(path.clone());
            }
            remove_dir_all_if_exists(&self.history_dir.join(id.to_string()))?;
            self.crash_check(CrashPoint::ForgetAfter2)?;

            // Step 3: navigation, once the caller is done (the index purge
            // and the event are the caller's).
            forgotten.insert(*id);
            outcomes.push(ForgetOutcome {
                id: *id,
                status: ForgetStatus::Forgotten,
            });
        }
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Recovery at open (design §6, the recovery column)
    // ------------------------------------------------------------------------

    /// Finishes or abandons an interrupted write from the single `pending`
    /// field, then deletes any tombstoned file still on disk. Idempotent.
    fn recover(&self) -> Result<(), MemoryFilesError> {
        let mut state = self.lock();
        let Some(current) = state.snapshot.as_ref() else {
            return Ok(());
        };
        // Read the snapshot first and unshare it (`Arc::make_mut`) only when
        // there is something to record: most opens have nothing to recover.
        let pending = current.pending.clone();
        let tombstoned: Vec<NoteId> = current.tombstones.iter().map(|t| t.id).collect();
        let mut changed = false;
        let mut records: Vec<(NoteId, NoteRecord)> = Vec::new();

        if let Some(pending) = &pending {
            changed = true;
            let files: Vec<(PathBuf, String)> = self
                .scan()?
                .into_iter()
                .filter(|e| e.id == Some(pending.id))
                .filter_map(|e| {
                    read_regular_nofollow(&e.path)
                        .ok()
                        .map(|bytes| (e.path, super::sha256_hex(&bytes)))
                })
                .collect();
            let at_to = files.iter().any(|(_, h)| *h == pending.to_rev);
            let all_at_from = files
                .iter()
                .all(|(_, h)| Some(h) == pending.from_rev.as_ref());
            if at_to {
                // Finish step 3. A retitle interrupted between its content
                // write and its rename may have left a `from_rev` twin.
                for (path, hash) in &files {
                    if *hash != pending.to_rev && Some(hash) == pending.from_rev.as_ref() {
                        remove_file_if_exists(path)?;
                    }
                }
                if let Some((path, _)) = files.iter().find(|(_, h)| *h == pending.to_rev)
                    && let Ok(bytes) = read_regular_nofollow(path)
                    && let Ok(note) = Note::parse(&bytes)
                {
                    let target = self.notes_dir.join(note.file_name());
                    if *path != target && !target.exists() {
                        let _ = fs::rename(path, &target);
                    }
                }
                records.push((
                    pending.id,
                    NoteRecord {
                        revision: pending.to_rev.clone(),
                        origin: pending.origin,
                        written_at: self.now(),
                    },
                ));
            } else if files.is_empty() || all_at_from {
                // Step 2 never landed: clear pending; the `.history` copy of
                // `to_rev` is bounded and harmless.
            } else if let [(_, hash)] = files.as_slice() {
                // An external edit raced the write: the note is `external` at
                // whatever the file holds; the service's own bytes are in
                // `.history`.
                records.push((
                    pending.id,
                    NoteRecord {
                        revision: hash.clone(),
                        origin: Origin::External,
                        written_at: self.now(),
                    },
                ));
            }
        }

        // Forget row: a tombstoned id whose file still exists is deleted.
        // One listing of `notes/` serves every tombstone.
        if !tombstoned.is_empty() {
            let ids: HashSet<NoteId> = tombstoned.iter().copied().collect();
            for entry in self.scan()? {
                if entry.id.is_some_and(|id| ids.contains(&id)) {
                    remove_file_if_exists(&entry.path)?;
                    changed = true;
                }
            }
        }
        let mut still_recorded = Vec::new();
        for id in tombstoned {
            let dir = self.history_dir.join(id.to_string());
            if remove_dir_all_if_exists(&dir)? {
                changed = true;
            }
            let recorded = records.iter().any(|(r, _)| *r == id)
                || state
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.notes.contains_key(&id));
            if recorded {
                still_recorded.push(id);
                changed = true;
            }
        }

        if changed {
            let snapshot = Self::snapshot_mut(&mut state);
            snapshot.pending = None;
            for (id, record) in records {
                snapshot.notes.insert(id, record);
            }
            for id in still_recorded {
                snapshot.notes.remove(&id);
            }
            let saved =
                self.save_snapshot(state.snapshot.as_mut().expect("checked above"), self.now())?;
            self.remember_snapshot(&state, saved);
            self.refresh_navigation(&state);
        }
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Reconciliation of external changes (design §5 *Freshness*)
    // ------------------------------------------------------------------------

    /// Re-hashes every note and brings the snapshot up to date with what
    /// happened behind the service's back: a new file is recorded `external`,
    /// an edited one re-recorded `external` at its current hash, a vanished
    /// one dropped. Returns what changed, for the index and `memory_changed`.
    /// A quarantined partition is refused; an absent one has nothing to do.
    pub fn reconcile(&self) -> Result<Vec<ReconcileChange>, MemoryFilesError> {
        self.reconcile_viewed().map(|(changes, _)| changes)
    }

    /// [`Partition::reconcile`], and the partition as it stands afterwards
    /// (design 09 §6.3 quick win 3), read once.
    pub fn reconcile_viewed(
        &self,
    ) -> Result<(Vec<ReconcileChange>, LoadedPartition), MemoryFilesError> {
        let mut state = self.lock();
        self.refuse_if_quarantined(&state)?;
        if state.snapshot.is_none() {
            let absent = LoadedPartition {
                health: PartitionHealth::Absent,
                views: Vec::new(),
                malformed: Vec::new(),
                duplicates: Vec::new(),
                conflicts: Vec::new(),
                scan: Vec::new(),
            };
            return Ok((Vec::new(), absent));
        }
        let (parsed, loaded) = self.load_parsed(&state)?;
        // Quick win 1: "gone" is three set lookups per recorded note, not
        // three list scans.
        let served: HashSet<NoteId> = loaded.views.iter().map(|v| v.note.id).collect();
        let present: HashSet<NoteId> = parsed.scan.iter().filter_map(|e| e.id).collect();
        let duplicated: HashSet<NoteId> = parsed.duplicates.iter().map(|(id, _)| *id).collect();
        let now = self.now();
        let recorded = state.snapshot.as_ref().expect("checked");
        let mut changes = Vec::new();
        for view in &loaded.views {
            let id = view.note.id;
            let kind = match recorded.notes.get(&id) {
                Some(record) if record.revision == view.revision => continue,
                Some(_) => ReconcileChangeKind::Modified,
                None => ReconcileChangeKind::Added,
            };
            changes.push(ReconcileChange {
                id,
                kind,
                revision: Some(view.revision.clone()),
            });
        }
        let gone: Vec<NoteId> = recorded
            .notes
            .keys()
            .filter(|id| !served.contains(id) && !present.contains(id) && !duplicated.contains(id))
            .copied()
            .collect();
        for id in gone {
            changes.push(ReconcileChange {
                id,
                kind: ReconcileChangeKind::Removed,
                revision: None,
            });
        }
        if changes.is_empty() {
            return Ok((changes, loaded));
        }
        // Only now is the snapshot unshared from the cache and changed.
        let snapshot = Self::snapshot_mut(&mut state);
        for change in &changes {
            match &change.revision {
                Some(revision) => {
                    snapshot.notes.insert(
                        change.id,
                        NoteRecord {
                            revision: revision.clone(),
                            origin: Origin::External,
                            written_at: now,
                        },
                    );
                }
                None => {
                    snapshot.notes.remove(&change.id);
                }
            }
        }
        let saved = self.save_snapshot(state.snapshot.as_mut().expect("checked"), now)?;
        self.remember_snapshot(&state, saved);
        let view = self.derive_view(&parsed, &state);
        self.write_navigation(&view.views);
        Ok((changes, view))
    }

    // ------------------------------------------------------------------------
    // Navigation
    // ------------------------------------------------------------------------

    fn refresh_navigation(&self, state: &State) {
        match self.load_with(state) {
            Ok(loaded) => self.write_navigation(&loaded.views),
            Err(e) => tracing::warn!("Could not rebuild {}: {e}", self.navigation_path.display()),
        }
    }

    /// `MEMORY.md`: ids, titles and relative links of the notes automatic
    /// recall may deliver — navigation, not another copy of facts (design §2).
    /// Best effort: a failure is logged, never an error.
    fn write_navigation(&self, views: &[NoteView]) {
        let mut listed: Vec<&NoteView> = views
            .iter()
            .filter(|v| v.derived.effective_status == EffectiveStatus::Active)
            .collect();
        listed.sort_by(|a, b| {
            a.note
                .title
                .cmp(&b.note.title)
                .then(a.note.id.cmp(&b.note.id))
        });
        let mut text = String::from(
            "# Memory\n\nGenerated navigation; rebuilt by the service, never edited by hand.\n\n",
        );
        for v in listed {
            text.push_str(&format!(
                "- [{}]({}/{}) — `{}` · {}\n",
                v.note.title.replace('[', "\\[").replace(']', "\\]"),
                NOTES_DIR,
                v.file_name,
                v.note.id,
                v.note.kind
            ));
        }
        if let Err(e) = write_atomic_private(&self.navigation_path, text.as_bytes()) {
            tracing::warn!("Could not write {}: {e:#}", self.navigation_path.display());
        }
    }

    fn refuse_if_quarantined(&self, state: &State) -> Result<(), MemoryFilesError> {
        if let Some(reason) = &state.quarantine {
            return Err(MemoryFilesError::Quarantined {
                partition_key: self.partition_key.clone(),
                reason: reason.clone(),
            });
        }
        Ok(())
    }

    // ------------------------------------------------------------------------
    // `doctor --repair` (design §6, *Management operations*)
    // ------------------------------------------------------------------------

    /// Rebuilds a missing or invalid snapshot from the directory and lifts the
    /// quarantine. Preconditions: the directory's owner key is the current
    /// owner's (else it is an orphan and `adopt` is the verb); no two parsed
    /// notes share an id — a duplicate refuses the repair naming both files,
    /// on a healthy partition too, because the service cannot choose.
    /// Postconditions: the binding from the path, every parsed note recorded
    /// at its current hash with origin `external`, no tombstones, no pending,
    /// `repaired_at` stamped. Never touches a note file. A partition that is
    /// not quarantined has its preconditions checked and nothing rebuilt.
    pub fn repair(&self, current_owner_key: &str) -> Result<RepairReport, MemoryFilesError> {
        let mut state = self.lock();
        if self.owner_key != current_owner_key {
            return Err(MemoryFilesError::InvalidInput(format!(
                "partition {} belongs to owner key {}, not the current owner's {current_owner_key}; `openalpaca memory adopt {}` is the verb for that",
                self.partition_key, self.owner_key, self.owner_key
            )));
        }
        let Parsed {
            notes: parsed,
            malformed,
            duplicates,
            ..
        } = self.parse_all()?;
        if let Some((id, files)) = duplicates.into_iter().next() {
            return Err(MemoryFilesError::DuplicateId { id, files });
        }
        if state.quarantine.is_none() {
            return Ok(RepairReport {
                partition_key: self.partition_key.clone(),
                rebuilt: false,
                notes_recorded: 0,
                malformed,
            });
        }
        let now = self.now();
        let created_at = ControlSnapshot::load(&self.snapshot_path)
            .ok()
            .flatten()
            .map(|s| s.created_at)
            .unwrap_or(now);
        let mut snapshot = ControlSnapshot::new(
            &self.partition_key,
            self.scope_kind,
            &self.owner_key,
            &self.owner_id,
            &self.root_canonical,
            created_at,
        );
        snapshot.repaired_at = Some(now);
        for (_, p) in &parsed {
            snapshot.notes.insert(
                p.note.id,
                NoteRecord {
                    revision: p.revision.clone(),
                    origin: Origin::External,
                    written_at: now,
                },
            );
        }
        create_private_dir(&self.history_dir)?;
        state_dir()?; // `state/` private before the snapshot's parent is made
        let mut snapshot = Arc::new(snapshot);
        let saved = self.save_snapshot(&mut snapshot, now)?;
        state.snapshot = Some(snapshot);
        state.quarantine = None;
        self.remember_snapshot(&state, saved);
        self.refresh_navigation(&state);
        Ok(RepairReport {
            partition_key: self.partition_key.clone(),
            rebuilt: true,
            notes_recorded: parsed.len(),
            malformed,
        })
    }
}

fn authority_conflict(id: NoteId, verb: &str) -> MemoryFilesError {
    MemoryFilesError::AuthorityConflict {
        id,
        message: format!(
            "Note {id} was saved by you, so a tool call may not {verb} it: open the Memory view in the app, or run `openalpaca memory edit {id}` (or `openalpaca memory add --supersedes {id}`)."
        ),
    }
}

// ============================================================================
// Home-store-wide operations (design §6, *Management operations*)
// ============================================================================

/// The owner keys that have a directory under `<store>/memory/owners/`.
pub fn list_owner_keys(scope: &StoreScope) -> Result<Vec<String>> {
    let owners = store_root(scope)?
        .join(ContentKind::Memory.dir_name())
        .join(OWNERS_DIR);
    let entries = match fs::read_dir(&owners) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("Failed to list {}", owners.display())),
    };
    let mut keys: Vec<String> = entries
        .flatten()
        .filter(|e| {
            fs::symlink_metadata(e.path())
                .map(|m| m.file_type().is_dir())
                .unwrap_or(false)
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| is_owner_key(name))
        .collect();
    keys.sort();
    Ok(keys)
}

/// Every snapshot file under `state/memory-control/`, parsed where it parses.
pub fn list_snapshots() -> Result<Vec<(PathBuf, Result<ControlSnapshot, String>)>> {
    let dir = home_root()?.join("state").join(CONTROL_DIR);
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("Failed to list {}", dir.display())),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        let parsed = match ControlSnapshot::load(&path) {
            Ok(Some(s)) => Ok(s),
            Ok(None) => continue,
            Err(e) => Err(e.to_string()),
        };
        out.push((path, parsed));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// What `doctor` says about a snapshot whose bound directory does not exist
/// (design §6, the workspace-move row; §7b P11).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MoveSnapshotState {
    /// Another snapshot names it in `moved_from` and that one's directory
    /// exists: the move completed; `--repair` deletes it.
    Stale,
    /// Its own `moved_from` names a snapshot whose directory exists: the move
    /// never happened; `--repair` deletes it.
    Abandoned,
    /// Listed, never deleted.
    Orphaned,
}

/// Classifies every snapshot whose `root` is not a directory. Pure over the
/// listing; the directory checks are the only I/O.
pub fn classify_move_snapshots(snapshots: &[ControlSnapshot]) -> Vec<(String, MoveSnapshotState)> {
    let dir_exists = |root: &str| Path::new(root).is_dir();
    let mut out = Vec::new();
    for s in snapshots {
        if dir_exists(&s.root) {
            continue;
        }
        let stale = snapshots.iter().any(|other| {
            other.moved_from.as_deref() == Some(s.partition_key.as_str()) && dir_exists(&other.root)
        });
        let abandoned = s.moved_from.as_deref().is_some_and(|from| {
            snapshots
                .iter()
                .any(|other| other.partition_key == from && dir_exists(&other.root))
        });
        let state = if stale {
            MoveSnapshotState::Stale
        } else if abandoned {
            MoveSnapshotState::Abandoned
        } else {
            MoveSnapshotState::Orphaned
        };
        out.push((s.partition_key.clone(), state));
    }
    out
}

/// One project snapshot a workspace move re-keys (design §3, *Workspace
/// move*): what step 2 wrote under the new key and what step 5 deletes under
/// the old one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotMove {
    pub old_key: String,
    pub new_key: String,
    pub old_path: PathBuf,
    pub new_path: PathBuf,
}

/// The project a partition directory belongs to:
/// `<project>/.openalpaca/memory/owners/<owner-key>` → `<project>`.
fn project_of_partition_root(root: &str) -> Option<PathBuf> {
    Path::new(root).ancestors().nth(4).map(Path::to_path_buf)
}

/// Step 2 of the workspace move (design §3): for every project partition of
/// `old_project` whose snapshot is readable, writes that snapshot's twin
/// under the **new** partition key — bound to the directory the partition
/// will have once the store is renamed, `moved_from` naming the old key,
/// notes, tombstones and `pending` copied — before anything is renamed. The
/// old snapshot is not touched. Returns one entry per partition re-keyed;
/// a project with no partition yields none, and writes nothing.
///
/// Until step 5 the twin is bound to a directory that does not exist yet,
/// which [`classify_move_snapshots`] reads as *abandoned* while the old
/// directory still stands, and the old snapshot as *stale* once the rename
/// has run — the two states a crash between the steps leaves for `doctor`.
///
/// A snapshot file that does not parse is skipped: it names no directory
/// this function could match, the partition it belongs to is quarantined
/// wherever its directory lands, and `doctor --repair` is its remedy.
pub fn begin_project_move(
    old_project: &Path,
    new_project: &Path,
    now: DateTime<Utc>,
) -> Result<Vec<SnapshotMove>, MemoryFilesError> {
    let old_project = PathBuf::from(canonical_string_lenient(old_project)?);
    let new_project = PathBuf::from(canonical_string_lenient(new_project)?);
    let mut moves = Vec::new();
    for (old_path, parsed) in list_snapshots()? {
        let snapshot = match parsed {
            Ok(s) if s.scope == SnapshotScope::Project => s,
            Ok(_) => continue,
            Err(reason) => {
                tracing::debug!(
                    path = %old_path.display(),
                    "memory: unreadable control snapshot left alone by the workspace move: {reason}"
                );
                continue;
            }
        };
        if project_of_partition_root(&snapshot.root).as_deref() != Some(old_project.as_path()) {
            continue;
        }
        // The same string `Partition::open` will compute after the rename:
        // the canonical project root plus components the rename carries over
        // unchanged (a link at any of them would refuse the partition anyway).
        let new_root = new_project
            .join(STORE_DIR_NAME)
            .join(ContentKind::Memory.dir_name())
            .join(OWNERS_DIR)
            .join(&snapshot.owner_key)
            .to_string_lossy()
            .into_owned();
        let new_key = project_partition_key(&snapshot.owner_key, &new_root);
        let new_path = snapshot_path_for(&new_key)?;
        let mut twin = snapshot.clone();
        twin.partition_key = new_key.clone();
        twin.root = new_root;
        twin.moved_from = Some(snapshot.partition_key.clone());
        twin.save(&new_path, now)?;
        moves.push(SnapshotMove {
            old_key: snapshot.partition_key,
            new_key,
            old_path,
            new_path,
        });
    }
    Ok(moves)
}

/// Step 5 of the workspace move (design §3): deletes each old-key snapshot,
/// then clears `moved_from` on its twin — in that order, so a crash between
/// the two leaves a twin bound to a directory that exists, whose
/// `moved_from` names a snapshot that is gone: nothing for
/// [`classify_move_snapshots`] to flag, and nothing left bound to a missing
/// directory. Idempotent: a finished move finishes again as a no-op.
pub fn finish_project_move(
    moves: &[SnapshotMove],
    now: DateTime<Utc>,
) -> Result<(), MemoryFilesError> {
    for mv in moves {
        remove_file_if_exists(&mv.old_path)?;
        let mut twin = match ControlSnapshot::load(&mv.new_path) {
            Ok(Some(twin)) => twin,
            Ok(None) => {
                return Err(MemoryFilesError::Io(anyhow::anyhow!(
                    "the re-keyed control snapshot {} is gone; run `openalpaca memory doctor`",
                    mv.new_path.display()
                )));
            }
            Err(e) => return Err(MemoryFilesError::Io(anyhow::anyhow!("{e}"))),
        };
        if twin.moved_from.is_some() {
            twin.moved_from = None;
            twin.save(&mv.new_path, now)?;
        }
    }
    Ok(())
}

/// `memory adopt <owner-key>` (design §6): re-binds every partition of an
/// orphaned owner key — the home partition and every `proj-<old>-*` snapshot
/// — to the current owner in one command. One collision refuses the whole
/// command; nothing is partially adopted. Returns the new partition keys.
///
/// `old_owner_key` is a directory name, never a path: anything but 32
/// lowercase hex characters is refused before it is joined to anything.
pub fn adopt(
    old_owner_key: &str,
    current_owner_id: &str,
    now: DateTime<Utc>,
) -> Result<Vec<String>, MemoryFilesError> {
    if !is_owner_key(old_owner_key) {
        return Err(MemoryFilesError::InvalidInput(format!(
            "`{old_owner_key}` is not an owner key (32 lowercase hex characters)"
        )));
    }
    let new_key = owner_key(current_owner_id);
    if old_owner_key == new_key {
        return Err(MemoryFilesError::InvalidInput(
            "that owner key is already the current owner's".into(),
        ));
    }
    let home = home_root()?;
    let control = home.join("state").join(CONTROL_DIR);

    // Every partition to adopt: (owners dir, old snapshot path, old snapshot).
    struct Candidate {
        owners_dir: PathBuf,
        old_snapshot: Option<(PathBuf, ControlSnapshot)>,
        scope: SnapshotScope,
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    let home_owners = home.join(ContentKind::Memory.dir_name()).join(OWNERS_DIR);
    let home_snapshot_path = control.join(format!("{}.json", home_partition_key(old_owner_key)));
    let home_snapshot = match ControlSnapshot::load(&home_snapshot_path) {
        Ok(s) => s,
        Err(e) => {
            return Err(MemoryFilesError::InvalidInput(format!(
                "the home snapshot of {old_owner_key} is unreadable ({e}); repair it first"
            )));
        }
    };
    if home_owners.join(old_owner_key).is_dir() || home_snapshot.is_some() {
        candidates.push(Candidate {
            owners_dir: home_owners.clone(),
            old_snapshot: home_snapshot.map(|s| (home_snapshot_path.clone(), s)),
            scope: SnapshotScope::Home,
        });
    }
    for (path, parsed) in list_snapshots()? {
        let prefix = format!("proj-{old_owner_key}-");
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.starts_with(&prefix) {
            continue;
        }
        let snapshot = parsed.map_err(|e| {
            MemoryFilesError::InvalidInput(format!(
                "{} is unreadable ({e}); repair it first",
                path.display()
            ))
        })?;
        let Some(owners_dir) = Path::new(&snapshot.root).parent().map(Path::to_path_buf) else {
            continue;
        };
        candidates.push(Candidate {
            owners_dir,
            old_snapshot: Some((path, snapshot)),
            scope: SnapshotScope::Project,
        });
    }
    if candidates.is_empty() {
        return Err(MemoryFilesError::InvalidInput(format!(
            "no partition of owner key {old_owner_key} exists"
        )));
    }

    // Collision check first, over everything, before anything moves.
    for c in &candidates {
        let target_dir = c.owners_dir.join(&new_key);
        if target_dir.exists() {
            return Err(MemoryFilesError::AdoptCollision(format!(
                "{} already exists",
                target_dir.display()
            )));
        }
        let new_partition_key = match c.scope {
            SnapshotScope::Home => home_partition_key(&new_key),
            SnapshotScope::Project => {
                project_partition_key(&new_key, &canonical_string_lenient(&target_dir)?)
            }
        };
        let new_snapshot_path = control.join(format!("{new_partition_key}.json"));
        if new_snapshot_path.exists() {
            return Err(MemoryFilesError::AdoptCollision(format!(
                "{} already exists",
                new_snapshot_path.display()
            )));
        }
    }

    state_dir()?; // `state/` private before the first snapshot is written
    let mut adopted = Vec::new();
    for c in candidates {
        let old_dir = c.owners_dir.join(old_owner_key);
        let new_dir = c.owners_dir.join(&new_key);
        if old_dir.is_dir() {
            fs::rename(&old_dir, &new_dir).with_context(|| {
                format!(
                    "Failed to rename {} to {}",
                    old_dir.display(),
                    new_dir.display()
                )
            })?;
            fsync_dir(&c.owners_dir)?;
        }
        let root_canonical = canonical_string_lenient(&new_dir)?;
        let new_partition_key = match c.scope {
            SnapshotScope::Home => home_partition_key(&new_key),
            SnapshotScope::Project => project_partition_key(&new_key, &root_canonical),
        };
        let mut snapshot = match &c.old_snapshot {
            Some((_, s)) => s.clone(),
            None => ControlSnapshot::new(
                &new_partition_key,
                c.scope,
                &new_key,
                current_owner_id,
                &root_canonical,
                now,
            ),
        };
        snapshot.partition_key = new_partition_key.clone();
        snapshot.owner_key = new_key.clone();
        snapshot.owner_id = current_owner_id.to_string();
        snapshot.root = root_canonical;
        snapshot.save(&control.join(format!("{new_partition_key}.json")), now)?;
        if let Some((old_path, _)) = &c.old_snapshot {
            remove_file_if_exists(old_path)?;
        }
        adopted.push(new_partition_key);
    }
    Ok(adopted)
}
