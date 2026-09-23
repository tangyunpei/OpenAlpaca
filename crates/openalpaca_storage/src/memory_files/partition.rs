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
use super::fsx::{
    ComponentState, canonical_string_lenient, create_private_dir, fsync_dir, read_regular_nofollow,
    real_directory_state, remove_dir_all_if_exists, remove_file_if_exists, write_atomic_private,
};
use super::note::{NewNote, Note, NoteId, NotePatch, NoteStatus, to_second};
use super::snapshot::{
    ControlSnapshot, NoteRecord, Origin, Pending, SnapshotError, SnapshotScope, Tombstone,
};
use super::{
    CONTROL_DIR, HISTORY_DIR, HISTORY_KEEP, MemoryFilesError, NAVIGATION_FILE, NOTES_DIR,
    OWNERS_DIR, PROJECT_GITIGNORE_TEXT, home_partition_key, is_revision, owner_key,
    project_partition_key,
};
use crate::store::{ContentKind, StoreScope, content_dir, home_root, store_root};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

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
}

/// `memory_update.action` (design §7a).
#[derive(Debug, Clone)]
pub enum UpdateAction {
    Edit(NotePatch),
    Archive,
    Confirm,
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

struct State {
    /// `None` until the first write lays the partition out, or while
    /// quarantined.
    snapshot: Option<ControlSnapshot>,
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

/// The snapshot file of a partition key.
pub fn snapshot_path_for(partition_key: &str) -> Result<PathBuf> {
    Ok(home_root()?
        .join("state")
        .join(CONTROL_DIR)
        .join(format!("{partition_key}.json")))
}

fn now_seconds() -> DateTime<Utc> {
    to_second(Utc::now())
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
        let store = store_root(scope)?;
        let key = owner_key(owner_id);
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
            #[cfg(test)]
            crash: Mutex::new(None),
        };

        if scope_kind == SnapshotScope::Project && memory_exists {
            partition.assert_gitignore()?;
        }

        let loaded = match ControlSnapshot::load(&partition.snapshot_path) {
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
        partition.recover()?;
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
        self.lock().snapshot.clone()
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
        let mut snapshot = ControlSnapshot::new(
            &self.partition_key,
            self.scope_kind,
            &self.owner_key,
            &self.owner_id,
            &self.root_canonical,
            self.now(),
        );
        snapshot.save(&self.snapshot_path, self.now())?;
        state.snapshot = Some(snapshot);
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
            #[cfg(unix)]
            let inode = {
                use std::os::unix::fs::MetadataExt;
                meta.ino()
            };
            #[cfg(not(unix))]
            let inode = 0u64;
            out.push(ScanEntry {
                id: Note::id_from_file_name(&file_name),
                path,
                file_name,
                size: meta.len(),
                mtime: meta.modified().ok(),
                inode,
            });
        }
        out.sort_by(|a, b| a.file_name.cmp(&b.file_name));
        Ok(out)
    }

    /// Parses every scanned file. Returns the parsed notes (one per file), the
    /// malformed files, and the duplicate ids.
    #[allow(clippy::type_complexity)]
    fn parse_all(
        &self,
    ) -> Result<(
        Vec<(ScanEntry, Note, String)>,
        Vec<MalformedFile>,
        Vec<(NoteId, Vec<PathBuf>)>,
    )> {
        let mut parsed = Vec::new();
        let mut malformed = Vec::new();
        for entry in self.scan()? {
            let bytes = match read_regular_nofollow(&entry.path) {
                Ok(b) => b,
                Err(e) => {
                    malformed.push(MalformedFile {
                        path: entry.path.clone(),
                        reason: format!("{e:#}"),
                    });
                    continue;
                }
            };
            match Note::parse(&bytes) {
                Ok(note) => {
                    if let Some(named) = entry.id
                        && named != note.id
                    {
                        malformed.push(MalformedFile {
                            path: entry.path.clone(),
                            reason: format!(
                                "the file name says {named} but the frontmatter id is {}",
                                note.id
                            ),
                        });
                        continue;
                    }
                    let revision = super::sha256_hex(&bytes);
                    parsed.push((entry, note, revision));
                }
                Err(e) => malformed.push(MalformedFile {
                    path: entry.path.clone(),
                    reason: e.reason,
                }),
            }
        }
        let mut by_id: BTreeMap<NoteId, Vec<PathBuf>> = BTreeMap::new();
        for (entry, note, _) in &parsed {
            by_id.entry(note.id).or_default().push(entry.path.clone());
        }
        let duplicates: Vec<(NoteId, Vec<PathBuf>)> = by_id
            .into_iter()
            .filter(|(_, paths)| paths.len() > 1)
            .collect();
        Ok((parsed, malformed, duplicates))
    }

    fn load_with(&self, state: &State) -> Result<LoadedPartition, MemoryFilesError> {
        let (parsed, malformed, duplicates) = self.parse_all()?;
        let dup_ids: Vec<NoteId> = duplicates.iter().map(|(id, _)| *id).collect();
        let unique: Vec<&(ScanEntry, Note, String)> = parsed
            .iter()
            .filter(|(_, note, _)| !dup_ids.contains(&note.id))
            .collect();
        let empty;
        let (snapshot, unverified) = match (&state.snapshot, &state.quarantine) {
            (Some(s), None) => (s, false),
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
            .map(|(_, note, revision)| NoteInput { note, revision })
            .collect();
        let Derivation {
            mut notes,
            conflicts,
        } = derive(&inputs, snapshot);
        let views = unique
            .into_iter()
            .map(|(entry, note, revision)| NoteView {
                derived: notes.remove(&note.id).expect("derived for every input"),
                note: note.clone(),
                revision: revision.clone(),
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
        Ok(LoadedPartition {
            health,
            views,
            malformed,
            duplicates,
            conflicts,
        })
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
    /// every point).
    fn commit_write(
        &self,
        state: &mut State,
        from_rev: Option<String>,
        note: &Note,
        origin: Origin,
        current_path: Option<&Path>,
    ) -> Result<String, MemoryFilesError> {
        self.ensure_laid_out(state)?;
        let bytes = note.serialize();
        let to_rev = super::sha256_hex(bytes.as_bytes());
        let snapshot = state.snapshot.as_mut().expect("laid out");

        // Step 1: the single pending field.
        snapshot.pending = Some(Pending {
            id: note.id,
            from_rev,
            to_rev: to_rev.clone(),
            origin,
        });
        snapshot.save(&self.snapshot_path, self.now())?;
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
        snapshot.notes.insert(
            note.id,
            NoteRecord {
                revision: to_rev.clone(),
                origin,
                written_at: self.now(),
            },
        );
        snapshot.pending = None;
        snapshot.save(&self.snapshot_path, self.now())?;
        self.crash_check(CrashPoint::WriteAfter3)?;

        // Step 4: navigation (the index and the event are the caller's).
        self.refresh_navigation(state);
        Ok(to_rev)
    }

    /// `memory_store` (design §7a): validates, checks a `supersedes` target's
    /// revision and authority, answers an exact duplicate with the existing
    /// id, else creates the note with a fresh UUID.
    pub fn create(&self, input: NewNote, actor: Actor) -> Result<Created, MemoryFilesError> {
        let mut state = self.lock();
        self.refuse_if_quarantined(&state)?;
        let Some(kind) = input.kind else {
            return Err(MemoryFilesError::InvalidInput("kind is required".into()));
        };
        let status = match input.status.unwrap_or(NoteStatus::Active) {
            NoteStatus::Archived => {
                return Err(MemoryFilesError::InvalidInput(
                    "a note is not created archived".into(),
                ));
            }
            s => s,
        };
        let loaded = self.load_with(&state)?;

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
            if view.derived.origin == Origin::User && !actor.is_user_directed() {
                return Err(authority_conflict(*target, "replace"));
            }
            supersedes.push(*target);
        } else if let Some(existing) = loaded.views.iter().find(|v| {
            v.derived.effective_status == EffectiveStatus::Active
                && v.note.kind == kind
                && v.note.title == input.title
                && v.note.body == input.body
                && v.note.fact_key == input.fact_key
                && v.note.aliases == input.aliases
                && v.note.tags == input.tags
        }) {
            return Ok(Created {
                id: existing.note.id,
                revision: existing.revision.clone(),
                origin: existing.derived.origin,
                dedup: Dedup::Existing,
                superseded: Vec::new(),
            });
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
        let revision = self.commit_write(&mut state, None, &note, origin, None)?;
        Ok(Created {
            id: note.id,
            revision,
            origin,
            dedup: Dedup::Created,
            superseded: supersedes,
        })
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
        let mut state = self.lock();
        self.refuse_if_quarantined(&state)?;
        let loaded = self.load_with(&state)?;
        let view = loaded.get(id).ok_or(MemoryFilesError::NotFound(*id))?;
        if view.revision != expected_revision {
            return Err(MemoryFilesError::RevisionConflict {
                id: *id,
                current_revision: view.revision.clone(),
            });
        }
        let mut new = view.note.clone();
        let origin = match &action {
            UpdateAction::Confirm => {
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
                new.status = NoteStatus::Active;
                Origin::User
            }
            UpdateAction::Archive => {
                if view.derived.origin == Origin::User && !actor.is_user_directed() {
                    return Err(authority_conflict(*id, "archive"));
                }
                if new.status == NoteStatus::Archived {
                    return Ok(Updated {
                        id: *id,
                        revision: view.revision.clone(),
                        status: new.status,
                        written: false,
                    });
                }
                new.status = NoteStatus::Archived;
                actor.origin()
            }
            UpdateAction::Edit(patch) => {
                if view.derived.origin == Origin::User && !actor.is_user_directed() {
                    return Err(authority_conflict(*id, "edit"));
                }
                patch.apply(&mut new);
                if new.serialize() == view.note.serialize() {
                    return Ok(Updated {
                        id: *id,
                        revision: view.revision.clone(),
                        status: new.status,
                        written: false,
                    });
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
        let revision = self.commit_write(&mut state, Some(from_rev), &new, origin, Some(&path))?;
        Ok(Updated {
            id: *id,
            revision,
            status,
            written: true,
        })
    }

    // ------------------------------------------------------------------------
    // Forget (design §6, tombstone first)
    // ------------------------------------------------------------------------

    /// `memory_forget` (design §7a): per target, a revision check, then the
    /// tombstone (with the note's origin, `supersedes` and source ids), then
    /// the file and its `.history`. Each target is its own single-note
    /// mutation. A target with no file answers `not_found` whatever revision
    /// was sent (§7b P12).
    pub fn forget(
        &self,
        targets: &[(NoteId, String)],
    ) -> Result<Vec<ForgetOutcome>, MemoryFilesError> {
        let mut state = self.lock();
        self.refuse_if_quarantined(&state)?;
        let mut outcomes = Vec::with_capacity(targets.len());
        for (id, expected) in targets {
            let loaded = self.load_with(&state)?;
            let Some(view) = loaded.get(id) else {
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
            let tombstone = Tombstone {
                id: *id,
                origin: view.derived.origin,
                supersedes: view.note.supersedes.clone(),
                suppressed_sources: view
                    .note
                    .sources
                    .iter()
                    .map(|s| s.suppression_key())
                    .collect(),
                forgotten_at: self.now(),
            };
            let path = view.path.clone();
            drop(loaded);
            let snapshot = state.snapshot.as_mut().expect("not quarantined, has notes");

            // Step 1: the tombstone, durable before anything is deleted.
            snapshot.tombstones.push(tombstone);
            snapshot.notes.remove(id);
            snapshot.save(&self.snapshot_path, self.now())?;
            self.crash_check(CrashPoint::ForgetAfter1)?;

            // Step 2: the file (every file with that id) and its history.
            remove_file_if_exists(&path)?;
            self.remove_files_for(id)?;
            remove_dir_all_if_exists(&self.history_dir.join(id.to_string()))?;
            self.crash_check(CrashPoint::ForgetAfter2)?;

            // Step 3: navigation (index purge and the event are the caller's).
            self.refresh_navigation(&state);
            outcomes.push(ForgetOutcome {
                id: *id,
                status: ForgetStatus::Forgotten,
            });
        }
        Ok(outcomes)
    }

    fn remove_files_for(&self, id: &NoteId) -> Result<()> {
        for entry in self.scan()? {
            if entry.id == Some(*id) {
                remove_file_if_exists(&entry.path)?;
            }
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
        let Some(snapshot) = state.snapshot.as_mut() else {
            return Ok(());
        };
        let mut changed = false;

        if let Some(pending) = snapshot.pending.take() {
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
                snapshot.notes.insert(
                    pending.id,
                    NoteRecord {
                        revision: pending.to_rev,
                        origin: pending.origin,
                        written_at: self.now(),
                    },
                );
            } else if files.is_empty() || all_at_from {
                // Step 2 never landed: clear pending; the `.history` copy of
                // `to_rev` is bounded and harmless.
            } else if let [(_, hash)] = files.as_slice() {
                // An external edit raced the write: the note is `external` at
                // whatever the file holds; the service's own bytes are in
                // `.history`.
                snapshot.notes.insert(
                    pending.id,
                    NoteRecord {
                        revision: hash.clone(),
                        origin: Origin::External,
                        written_at: self.now(),
                    },
                );
            }
        }

        // Forget row: a tombstoned id whose file still exists is deleted.
        let tombstoned: Vec<NoteId> = snapshot.tombstones.iter().map(|t| t.id).collect();
        for id in tombstoned {
            let dir = self.history_dir.join(id.to_string());
            let had_files = self.scan()?.iter().any(|e| e.id == Some(id));
            if had_files {
                self.remove_files_for(&id)?;
                changed = true;
            }
            if remove_dir_all_if_exists(&dir)? {
                changed = true;
            }
            if snapshot.notes.remove(&id).is_some() {
                changed = true;
            }
        }

        if changed {
            snapshot.save(&self.snapshot_path, self.now())?;
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
        let mut state = self.lock();
        self.refuse_if_quarantined(&state)?;
        if state.snapshot.is_none() {
            return Ok(Vec::new());
        }
        let loaded = self.load_with(&state)?;
        let present_ids: Vec<NoteId> = self.scan()?.iter().filter_map(|e| e.id).collect();
        let dup_ids: Vec<NoteId> = loaded.duplicates.iter().map(|(id, _)| *id).collect();
        let now = self.now();
        let snapshot = state.snapshot.as_mut().expect("checked");
        let mut changes = Vec::new();
        for view in &loaded.views {
            let id = view.note.id;
            match snapshot.notes.get(&id) {
                Some(record) if record.revision == view.revision => {}
                Some(_) => {
                    snapshot.notes.insert(
                        id,
                        NoteRecord {
                            revision: view.revision.clone(),
                            origin: Origin::External,
                            written_at: now,
                        },
                    );
                    changes.push(ReconcileChange {
                        id,
                        kind: ReconcileChangeKind::Modified,
                        revision: Some(view.revision.clone()),
                    });
                }
                None => {
                    snapshot.notes.insert(
                        id,
                        NoteRecord {
                            revision: view.revision.clone(),
                            origin: Origin::External,
                            written_at: now,
                        },
                    );
                    changes.push(ReconcileChange {
                        id,
                        kind: ReconcileChangeKind::Added,
                        revision: Some(view.revision.clone()),
                    });
                }
            }
        }
        let served: Vec<NoteId> = loaded.views.iter().map(|v| v.note.id).collect();
        let gone: Vec<NoteId> = snapshot
            .notes
            .keys()
            .filter(|id| !served.contains(id) && !present_ids.contains(id) && !dup_ids.contains(id))
            .copied()
            .collect();
        for id in gone {
            snapshot.notes.remove(&id);
            changes.push(ReconcileChange {
                id,
                kind: ReconcileChangeKind::Removed,
                revision: None,
            });
        }
        if !changes.is_empty() {
            snapshot.save(&self.snapshot_path, now)?;
            self.refresh_navigation(&state);
        }
        Ok(changes)
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
        let (parsed, malformed, duplicates) = self.parse_all()?;
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
        for (_, note, revision) in &parsed {
            snapshot.notes.insert(
                note.id,
                NoteRecord {
                    revision: revision.clone(),
                    origin: Origin::External,
                    written_at: now,
                },
            );
        }
        create_private_dir(&self.history_dir)?;
        snapshot.save(&self.snapshot_path, now)?;
        state.snapshot = Some(snapshot);
        state.quarantine = None;
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
        .filter(|name| {
            name.len() == super::OWNER_KEY_HEX
                && name
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
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

/// `memory adopt <owner-key>` (design §6): re-binds every partition of an
/// orphaned owner key — the home partition and every `proj-<old>-*` snapshot
/// — to the current owner in one command. One collision refuses the whole
/// command; nothing is partially adopted. Returns the new partition keys.
pub fn adopt(
    old_owner_key: &str,
    current_owner_id: &str,
    now: DateTime<Utc>,
) -> Result<Vec<String>, MemoryFilesError> {
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
