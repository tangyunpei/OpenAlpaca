//! The parse cache (design 09 §6.3): the notes a partition read parsed, and
//! the control snapshot it opened, kept between reads — so reading a
//! partition nothing changed re-reads no file.
//!
//! - **Key:** `(partition_key, file_name)` for a note; the partition key for
//!   its snapshot. Every entry also remembers the directory or file it was
//!   read from, so one owner's key under two home stores is never confused.
//! - **Valid while** the file's stat identity — `(mtime_ns, size, inode,
//!   ctime, nlink)` — is unchanged: what the reconciliation scan compares,
//!   plus `ctime`, which no user tool can set back, and the link count, so a
//!   file that gained a hard link (refused by the read) is read again. A
//!   snapshot entry is also bound to the save generation of this process
//!   ([`super::snapshot::save_generation`]), which every save bumps.
//! - **Recently changed files are re-hashed** ("racy" entries, as git calls
//!   them): a file whose stat was taken within [`RACY_WINDOW`] of its last
//!   change could be changed again inside the same file-system timestamp
//!   tick without its stat moving. Such an entry — and every entry a write
//!   of this process puts in — is used only after the file's bytes are read
//!   and hash to its revision; once its stat is old enough it is trusted.
//! - **Bound:** at most [`NOTE_PARSE_CACHE_BYTES`] of parsed notes, body
//!   bytes plus metadata, **all or nothing per partition** — a partition that
//!   would not fit is parsed uncached — with least-recently-used eviction
//!   across partitions. Snapshots are not counted: there is one per
//!   partition the service opened. A cache of capacity 0 holds nothing.
//! - **The invariant** is the partition's, not the cache's: a mutation
//!   re-reads and re-hashes each file it checks a revision against before the
//!   check (`Partition::recheck`), so a cached parse never stands in for the
//!   bytes a write replaces.
//!
//! The stat function is a parameter ([`NoteParseCache::with_stat`]),
//! defaulting to the partition scan's own `lstat`, because a real edit
//! cannot keep a file's stat: a test injects one that reports the pre-edit
//! key.

use super::note::{Note, NoteId, NoteSource, UnknownField};
use super::snapshot::ControlSnapshot;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The cap on cached notes (design 09 §6.3): 128 MiB, body bytes plus
/// metadata — about 70 000 notes of 1.3 KB, or 180 000 learned-size ones.
pub const NOTE_PARSE_CACHE_BYTES: usize = 128 * 1024 * 1024;

/// How long after its last change a file's stat still cannot vouch for its
/// bytes: well over a Linux file-timestamp tick (at most 10 ms). A file
/// whose timestamps have no sub-second part is on a file system that keeps
/// whole seconds, and waits [`RACY_WINDOW_COARSE`] instead.
pub const RACY_WINDOW: Duration = Duration::from_millis(100);
/// [`RACY_WINDOW`] for whole-second timestamps (FAT keeps two).
pub const RACY_WINDOW_COARSE: Duration = Duration::from_secs(3);

/// A file's stat identity: what must be unchanged for a cached parse to
/// stand for its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StatKey {
    pub mtime_ns: i128,
    pub size: u64,
    /// `0` where the platform has none.
    pub inode: u64,
    /// The inode's change time; the modification time where the platform
    /// has none.
    pub ctime_ns: i128,
    pub nlink: u64,
}

impl StatKey {
    /// The key of `symlink_metadata`.
    pub fn of(meta: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                mtime_ns: i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()),
                size: meta.size(),
                inode: meta.ino(),
                ctime_ns: i128::from(meta.ctime()) * 1_000_000_000 + i128::from(meta.ctime_nsec()),
                nlink: meta.nlink(),
            }
        }
        #[cfg(not(unix))]
        {
            let mtime_ns = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i128)
                .unwrap_or(0);
            Self {
                mtime_ns,
                size: meta.len(),
                inode: 0,
                ctime_ns: mtime_ns,
                nlink: 1,
            }
        }
    }

    /// The real `lstat` — what [`NoteParseCache::new`] uses. `None` when the
    /// path cannot be stat'ed.
    pub fn lstat(path: &Path) -> Option<Self> {
        fs::symlink_metadata(path).ok().map(|m| Self::of(&m))
    }

    /// Whether a stat taken no earlier than `taken` can vouch for the bytes:
    /// the file last changed more than `window` before it — or, for a
    /// change time with no sub-second part, more than
    /// [`RACY_WINDOW_COARSE`]. A change time after `taken` (a clock that
    /// moved) never vouches.
    pub fn settled(&self, taken: SystemTime, window: Duration) -> bool {
        let Ok(taken) = taken.duration_since(UNIX_EPOCH) else {
            return false;
        };
        let window = if self.ctime_ns.rem_euclid(1_000_000_000) == 0 {
            window.max(RACY_WINDOW_COARSE)
        } else {
            window
        };
        taken.as_nanos() as i128 - self.ctime_ns > window.as_nanos() as i128
    }
}

/// A stat function the cache can be given in place of the scan's `lstat`.
pub type StatFn = Arc<dyn Fn(&Path) -> Option<StatKey> + Send + Sync>;

/// One parsed note file: the note and the revision of its bytes.
#[derive(Debug)]
pub(super) struct ParsedNote {
    pub note: Note,
    pub revision: String,
}

/// A cached note file.
#[derive(Debug, Clone)]
pub(super) struct FileEntry {
    pub stat: StatKey,
    pub parsed: Arc<ParsedNote>,
    /// `false` until a read after the racy window has seen the file's
    /// bytes hash to the revision: until then a hit re-reads and re-hashes.
    pub trusted: bool,
    pub bytes: usize,
}

impl FileEntry {
    pub fn new(file_name: &str, stat: StatKey, parsed: Arc<ParsedNote>, trusted: bool) -> Self {
        Self {
            bytes: note_bytes(file_name, &parsed),
            stat,
            parsed,
            trusted,
        }
    }
}

/// What a cached note costs, counted the design's way: its body and every
/// other string it holds, plus the fixed size of the structures around it.
fn note_bytes(file_name: &str, parsed: &ParsedNote) -> usize {
    const ENTRY_OVERHEAD: usize = 128;
    let n = &parsed.note;
    let list = |items: &[String]| {
        items
            .iter()
            .map(|s| s.len() + std::mem::size_of::<String>())
            .sum::<usize>()
    };
    ENTRY_OVERHEAD
        + std::mem::size_of::<FileEntry>()
        + std::mem::size_of::<ParsedNote>()
        + file_name.len()
        + parsed.revision.len()
        + n.title.len()
        + n.body.len()
        + n.fact_key.as_ref().map_or(0, String::len)
        + list(&n.aliases)
        + list(&n.tags)
        + n.sources
            .iter()
            .map(|s| s.id.len() + std::mem::size_of::<NoteSource>())
            .sum::<usize>()
        + n.supersedes.len() * std::mem::size_of::<NoteId>()
        + n.unknown
            .iter()
            .map(|u| u.key.len() + u.raw.len() + std::mem::size_of::<UnknownField>())
            .sum::<usize>()
}

/// A cached control snapshot.
#[derive(Debug, Clone)]
struct SnapshotEntry {
    path: PathBuf,
    stat: StatKey,
    generation: u64,
    snapshot: Arc<ControlSnapshot>,
}

/// One partition's entries.
#[derive(Debug, Default)]
struct Slot {
    /// The `notes/` directory the files were read from.
    notes_dir: PathBuf,
    /// `None`: the partition's notes are not cached — never read yet, or
    /// over the cap, or evicted.
    files: Option<Arc<HashMap<String, FileEntry>>>,
    bytes: usize,
    last_used: u64,
    snapshot: Option<SnapshotEntry>,
}

#[derive(Debug, Default)]
struct Inner {
    clock: u64,
    /// The bytes of every partition whose notes are cached.
    total: usize,
    slots: HashMap<String, Slot>,
    /// The partitions — key and `notes/` directory — whose forgotten files
    /// recovery at open has swept in this process (scale slice S3): not a
    /// parse, so kept whatever the capacity, and forgotten by `clear`,
    /// `evict_partition` and a forget that failed part-way.
    swept: HashSet<(String, PathBuf)>,
}

/// The parse cache the service owns (design 09 §6.3) and hands to every
/// partition it opens.
pub struct NoteParseCache {
    capacity: usize,
    stat: Option<StatFn>,
    racy_window: Duration,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for NoteParseCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoteParseCache")
            .field("capacity", &self.capacity)
            .field("cached_bytes", &self.cached_bytes())
            .finish()
    }
}

impl Default for NoteParseCache {
    fn default() -> Self {
        Self::new(NOTE_PARSE_CACHE_BYTES)
    }
}

impl NoteParseCache {
    /// A cache of at most `capacity` bytes of parsed notes; 0 caches
    /// nothing, snapshots included — every read is today's full parse.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            stat: None,
            racy_window: RACY_WINDOW,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Replaces the stat function (tests): every stat the cache compares
    /// or records is `stat`'s.
    pub fn with_stat(
        mut self,
        stat: impl Fn(&Path) -> Option<StatKey> + Send + Sync + 'static,
    ) -> Self {
        self.stat = Some(Arc::new(stat));
        self
    }

    /// Replaces [`RACY_WINDOW`] (tests): an hour keeps every entry
    /// re-hashed on each read, zero trusts a file at its first read.
    pub fn with_racy_window(mut self, window: Duration) -> Self {
        self.racy_window = window;
        self
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Whether `stat`, taken no earlier than `taken`, vouches for the bytes
    /// under this cache's racy window.
    pub(super) fn settled(&self, stat: &StatKey, taken: SystemTime) -> bool {
        stat.settled(taken, self.racy_window)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The key of `path`: the injected function's, else `scanned` — the
    /// partition scan's own `lstat` of it.
    pub(super) fn key(&self, path: &Path, scanned: Option<StatKey>) -> Option<StatKey> {
        match &self.stat {
            Some(stat) => stat(path),
            None => scanned,
        }
    }

    /// Bytes of parsed notes held now, over every partition.
    pub fn cached_bytes(&self) -> usize {
        self.lock().total
    }

    /// Whether `partition_key`'s notes are cached now, and how many files.
    pub fn cached_files(&self, partition_key: &str) -> Option<usize> {
        let inner = self.lock();
        inner
            .slots
            .get(partition_key)
            .and_then(|s| s.files.as_ref())
            .map(|f| f.len())
    }

    /// Drops everything cached for `partition_key` — a workspace move
    /// re-keys a partition, and the old key's entries must go with it.
    pub fn evict_partition(&self, partition_key: &str) {
        let mut inner = self.lock();
        if let Some(slot) = inner.slots.remove(partition_key) {
            inner.total -= slot.bytes;
        }
        inner.swept.retain(|(key, _)| key != partition_key);
    }

    /// Drops everything (an index rebuild).
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.slots.clear();
        inner.total = 0;
        inner.swept.clear();
    }

    // ------------------------------------------------------------------------
    // Recovery's sweep of forgotten files (scale slice S3)
    // ------------------------------------------------------------------------

    /// Whether recovery has swept this partition's forgotten files in this
    /// process — so an open need not list `notes/` for them again.
    pub(super) fn tombstones_swept(&self, partition_key: &str, notes_dir: &Path) -> bool {
        self.lock()
            .swept
            .contains(&(partition_key.to_string(), notes_dir.to_path_buf()))
    }

    /// Records a sweep that completed.
    pub(super) fn mark_tombstones_swept(&self, partition_key: &str, notes_dir: &Path) {
        self.lock()
            .swept
            .insert((partition_key.to_string(), notes_dir.to_path_buf()));
    }

    /// The next open sweeps again: a forget failed after its tombstone and
    /// may have left the note's files behind.
    pub(super) fn rearm_tombstone_sweep(&self, partition_key: &str, notes_dir: &Path) {
        self.lock()
            .swept
            .remove(&(partition_key.to_string(), notes_dir.to_path_buf()));
    }

    // ------------------------------------------------------------------------
    // Notes
    // ------------------------------------------------------------------------

    /// The partition's cached files, when they were read from `notes_dir`.
    pub(super) fn files(
        &self,
        partition_key: &str,
        notes_dir: &Path,
    ) -> Option<Arc<HashMap<String, FileEntry>>> {
        if self.capacity == 0 {
            return None;
        }
        let mut inner = self.lock();
        inner.clock += 1;
        let clock = inner.clock;
        let slot = inner.slots.get_mut(partition_key)?;
        if slot.notes_dir != notes_dir {
            return None;
        }
        slot.last_used = clock;
        slot.files.clone()
    }

    /// Replaces the partition's files with what one read found — or, when
    /// they would not fit under the cap, leaves the partition uncached —
    /// and evicts the least recently used other partitions until the cache
    /// fits. `files` is `None` when the read already knew they would not fit.
    pub(super) fn store_files(
        &self,
        partition_key: &str,
        notes_dir: &Path,
        files: Option<HashMap<String, FileEntry>>,
    ) {
        if self.capacity == 0 {
            return;
        }
        let sum: usize = files
            .as_ref()
            .map_or(0, |f| f.values().map(|e| e.bytes).sum());
        let (files, bytes) = if sum <= self.capacity {
            (files, sum)
        } else {
            (None, 0)
        };
        let mut inner = self.lock();
        inner.clock += 1;
        let clock = inner.clock;
        let slot = Self::slot_for(&mut inner, partition_key, notes_dir);
        let old = std::mem::replace(&mut slot.bytes, bytes);
        slot.files = files.map(Arc::new);
        slot.last_used = clock;
        inner.total = inner.total - old + bytes;
        self.evict_to_fit(&mut inner, partition_key);
    }

    /// The partition's slot, bound to `notes_dir`: a slot read from another
    /// directory under the same key (another home store) starts over.
    fn slot_for<'a>(inner: &'a mut Inner, partition_key: &str, notes_dir: &Path) -> &'a mut Slot {
        let elsewhere = inner
            .slots
            .get(partition_key)
            .is_some_and(|slot| slot.notes_dir != notes_dir);
        if elsewhere && let Some(stale) = inner.slots.remove(partition_key) {
            inner.total -= stale.bytes;
        }
        let slot = inner.slots.entry(partition_key.to_string()).or_default();
        if slot.notes_dir.as_os_str().is_empty() {
            slot.notes_dir = notes_dir.to_path_buf();
        }
        slot
    }

    /// Puts the file a write of this process just made — untrusted, so the
    /// next read re-hashes it — in place of `replaced` (the name it moved
    /// from, under a retitle). Only a partition whose notes are cached takes
    /// it: an uncached one is read whole next time anyway.
    pub(super) fn put_file(
        &self,
        partition_key: &str,
        notes_dir: &Path,
        replaced: &[String],
        file_name: &str,
        entry: FileEntry,
    ) {
        self.update_files(partition_key, notes_dir, |files| {
            let mut delta = entry.bytes as isize;
            for name in replaced {
                if let Some(old) = files.remove(name) {
                    delta -= old.bytes as isize;
                }
            }
            if let Some(old) = files.insert(file_name.to_string(), entry) {
                delta -= old.bytes as isize;
            }
            delta
        });
    }

    /// Drops the named files from the partition's entries (a forget).
    pub(super) fn remove_files(&self, partition_key: &str, notes_dir: &Path, names: &[String]) {
        self.update_files(partition_key, notes_dir, |files| {
            let mut delta = 0isize;
            for name in names {
                if let Some(old) = files.remove(name) {
                    delta -= old.bytes as isize;
                }
            }
            delta
        });
    }

    /// Applies `change` to the partition's cached files — when they are
    /// cached, and read from `notes_dir` — and accounts the bytes it says it
    /// added (or, negative, freed).
    fn update_files(
        &self,
        partition_key: &str,
        notes_dir: &Path,
        change: impl FnOnce(&mut HashMap<String, FileEntry>) -> isize,
    ) {
        if self.capacity == 0 {
            return;
        }
        let mut inner = self.lock();
        let Some(slot) = inner.slots.get_mut(partition_key) else {
            return;
        };
        if slot.notes_dir != notes_dir {
            return;
        }
        let Some(files) = slot.files.as_mut() else {
            return;
        };
        let delta = change(Arc::make_mut(files));
        let mut bytes = slot.bytes.saturating_add_signed(delta);
        if bytes > self.capacity {
            slot.files = None;
            bytes = 0;
        }
        let old = std::mem::replace(&mut slot.bytes, bytes);
        inner.total = inner.total - old + bytes;
        self.evict_to_fit(&mut inner, partition_key);
    }

    /// Evicts the least recently used partitions other than `keep` until
    /// the cached notes fit under the cap.
    fn evict_to_fit(&self, inner: &mut Inner, keep: &str) {
        while inner.total > self.capacity {
            let victim = inner
                .slots
                .iter()
                .filter(|(key, slot)| key.as_str() != keep && slot.files.is_some())
                .min_by_key(|(_, slot)| slot.last_used)
                .map(|(key, _)| key.clone());
            let Some(victim) = victim else {
                break;
            };
            let slot = inner.slots.get_mut(&victim).expect("just found");
            slot.files = None;
            let freed = std::mem::take(&mut slot.bytes);
            inner.total -= freed;
        }
    }

    // ------------------------------------------------------------------------
    // Snapshots
    // ------------------------------------------------------------------------

    /// The partition's cached snapshot, when it was read from `path`, the
    /// file's stat is still `stat` and no save of this process came since.
    pub(super) fn snapshot(
        &self,
        partition_key: &str,
        notes_dir: &Path,
        path: &Path,
        stat: StatKey,
        generation: u64,
    ) -> Option<Arc<ControlSnapshot>> {
        if self.capacity == 0 {
            return None;
        }
        let inner = self.lock();
        let slot = inner.slots.get(partition_key)?;
        let entry = slot.snapshot.as_ref()?;
        (slot.notes_dir == notes_dir
            && entry.path == path
            && entry.stat == stat
            && entry.generation == generation)
            .then(|| Arc::clone(&entry.snapshot))
    }

    /// Records the snapshot at `path` as it stands at `stat` and
    /// `generation`.
    pub(super) fn put_snapshot(
        &self,
        partition_key: &str,
        notes_dir: &Path,
        path: &Path,
        stat: StatKey,
        generation: u64,
        snapshot: Arc<ControlSnapshot>,
    ) {
        if self.capacity == 0 {
            return;
        }
        let mut inner = self.lock();
        let slot = Self::slot_for(&mut inner, partition_key, notes_dir);
        slot.snapshot = Some(SnapshotEntry {
            path: path.to_path_buf(),
            stat,
            generation,
            snapshot,
        });
    }
}
