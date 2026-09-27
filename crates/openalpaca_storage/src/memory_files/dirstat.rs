//! The stat scan's directory listing (design §5 *Freshness*; 03 §7c R3):
//! every regular `*.md` file of `notes/` that does not start with a dot,
//! with the facts the scan and the parse cache compare — its size, its
//! modification and change times to the nanosecond, its inode and its link
//! count ([`ScanEntry`], [`StatKey`]).
//!
//! Owner decision LQ16 (2026-09-26) keeps R3 — a stat scan before every
//! read — and makes it cheaper where the platform can: on macOS one
//! `getattrlistbulk(2)` pass returns every entry's name, type and stat
//! fields in batches, where `readdir` then one `lstat` per file costs a
//! system call per note (measured 87 ms against 156 ms over 100 000 files,
//! L8 `bench/scale/sceptic/scanbench.c`). Elsewhere it is `readdir` and
//! `lstat`, as before. Both answer the same entries with the same fields,
//! bit for bit: [`list_by_lstat`] is the reference, and a test runs both
//! over one directory.

use super::cache::StatKey;
use super::note::Note;
use super::partition::ScanEntry;
use std::fs;
use std::io;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Whether a directory entry's name is a note file the scan lists: `*.md`,
/// not hidden (a temp file or the store's own), whatever its type — the
/// caller keeps regular files only.
pub(super) fn is_note_name(name: &str) -> bool {
    !name.starts_with('.') && name.ends_with(".md")
}

/// The note files of `dir` with their stat facts, unsorted: `Ok(None)` when
/// `dir` does not exist. The platform's fastest listing ([`list_bulk`] on
/// macOS, [`list_by_lstat`] elsewhere); an entry that vanished between the
/// listing and its stat is left out, as a scan meeting it later would.
pub(super) fn list_notes(dir: &Path) -> io::Result<Option<Vec<(ScanEntry, StatKey)>>> {
    #[cfg(target_os = "macos")]
    {
        list_bulk(dir)
    }
    #[cfg(not(target_os = "macos"))]
    {
        list_by_lstat(dir)
    }
}

/// `readdir`, then one `lstat` per note name: the reference listing.
#[cfg(any(not(target_os = "macos"), test))]
pub(super) fn list_by_lstat(dir: &Path) -> io::Result<Option<Vec<(ScanEntry, StatKey)>>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !is_note_name(&file_name) {
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
    Ok(Some(out))
}

/// One entry from its `symlink_metadata`.
pub(super) fn scan_entry(
    path: std::path::PathBuf,
    file_name: String,
    meta: &fs::Metadata,
) -> ScanEntry {
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

/// `getattrlistbulk(2)`: the directory's entries in batches, each with its
/// name, object type, modification and change times, file id, link count
/// and data length — the fields `lstat` gives [`scan_entry`] and
/// [`StatKey::of`], which `getattrlist(2)` documents as the same values
/// (`ATTR_CMN_FILEID` is `st_ino`, `ATTR_FILE_DATALENGTH` the data fork's
/// logical length, `st_size`). An entry the call could not describe fully
/// is stat'ed on its own, as [`list_by_lstat`] would.
#[cfg(target_os = "macos")]
pub(super) fn list_bulk(dir: &Path) -> io::Result<Option<Vec<(ScanEntry, StatKey)>>> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    let handle = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY)
        .open(dir)
    {
        Ok(h) => h,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut attrs = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_RETURNED_ATTRS
            | libc::ATTR_CMN_NAME
            | libc::ATTR_CMN_OBJTYPE
            | libc::ATTR_CMN_MODTIME
            | libc::ATTR_CMN_CHGTIME
            | libc::ATTR_CMN_FILEID,
        volattr: 0,
        dirattr: 0,
        fileattr: libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_DATALENGTH,
        forkattr: 0,
    };
    // A u64 buffer, so each batch starts 8-byte aligned; the records inside
    // are packed on 4-byte boundaries and read byte by byte below.
    let mut buffer = vec![0u64; BULK_BUFFER_BYTES / 8];
    let mut out = Vec::new();
    loop {
        // SAFETY: `handle` is an open directory for the whole call, `attrs`
        // is a valid `attrlist` the call only reads, and the buffer pointer
        // and its length in bytes describe one live, writable allocation.
        // FSOPT_PACK_INVAL_ATTRS makes every record carry every requested
        // attribute, the invalid ones zeroed and flagged in the returned set.
        let count = unsafe {
            libc::getattrlistbulk(
                handle.as_raw_fd(),
                (&raw mut attrs).cast(),
                buffer.as_mut_ptr().cast(),
                buffer.len() * 8,
                u64::from(libc::FSOPT_PACK_INVAL_ATTRS),
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            return Ok(Some(out));
        }
        let bytes: &[u8] = bytemuck_bytes(&buffer);
        let mut at = 0usize;
        for _ in 0..count {
            let record = Record::parse(bytes, at)
                .ok_or_else(|| io::Error::other("getattrlistbulk returned a short record"))?;
            at += record.length;
            if !is_note_name(&record.name) {
                continue;
            }
            let path = dir.join(&record.name);
            match record.stat() {
                // Not a regular file: a link, a directory.
                Some(None) => {}
                Some(Some(key)) => {
                    let entry = ScanEntry {
                        id: Note::id_from_file_name(&record.name),
                        path,
                        file_name: record.name,
                        size: key.size,
                        mtime: Some(system_time(record.mtime)),
                        inode: key.inode,
                    };
                    out.push((entry, key));
                }
                // Not every attribute came back: the file on its own, as the
                // reference listing reads it.
                None => {
                    let Ok(meta) = fs::symlink_metadata(&path) else {
                        continue;
                    };
                    if meta.file_type().is_file() {
                        out.push((scan_entry(path, record.name, &meta), StatKey::of(&meta)));
                    }
                }
            }
        }
    }
}

/// One batch holds this many bytes of records: a few thousand notes.
#[cfg(target_os = "macos")]
const BULK_BUFFER_BYTES: usize = 256 * 1024;

/// `VREG` of `<sys/vnode.h>`'s `enum vtype`: a regular file.
#[cfg(target_os = "macos")]
const VREG: u32 = 1;

/// The buffer's bytes.
#[cfg(target_os = "macos")]
fn bytemuck_bytes(words: &[u64]) -> &[u8] {
    // SAFETY: a `u64` slice is valid, initialised memory of `len * 8` bytes,
    // and `u8` has no alignment or validity requirement.
    unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * 8) }
}

/// `st_mtimespec` as `Metadata::modified` makes it: seconds may be negative,
/// nanoseconds never are.
#[cfg(target_os = "macos")]
fn system_time((sec, nsec): (i64, i64)) -> SystemTime {
    let nanos = Duration::from_nanos(nsec as u64);
    if sec >= 0 {
        UNIX_EPOCH + Duration::from_secs(sec as u64) + nanos
    } else {
        UNIX_EPOCH - Duration::from_secs(sec.unsigned_abs()) + nanos
    }
}

/// One `getattrlistbulk` record, as packed for the attributes
/// [`list_bulk`] asks for: the record's length, the returned attribute
/// set, then — in bit order, each on a 4-byte boundary — the name's
/// reference, the object type, the modification and change times, the
/// file id, then the file attributes: the link count and the data length.
#[cfg(target_os = "macos")]
struct Record {
    length: usize,
    returned_common: u32,
    returned_file: u32,
    name: String,
    kind: u32,
    mtime: (i64, i64),
    ctime: (i64, i64),
    file_id: u64,
    link_count: u32,
    data_length: i64,
}

#[cfg(target_os = "macos")]
impl Record {
    fn parse(bytes: &[u8], at: usize) -> Option<Self> {
        let u32_at = |o: usize| -> Option<u32> {
            Some(u32::from_ne_bytes(bytes.get(o..o + 4)?.try_into().ok()?))
        };
        let i32_at = |o: usize| -> Option<i32> {
            Some(i32::from_ne_bytes(bytes.get(o..o + 4)?.try_into().ok()?))
        };
        let i64_at = |o: usize| -> Option<i64> {
            Some(i64::from_ne_bytes(bytes.get(o..o + 8)?.try_into().ok()?))
        };
        let length = u32_at(at)? as usize;
        let record = bytes.get(at..at.checked_add(length)?)?;
        // `attribute_set_t`: common, vol, dir, file, fork.
        let returned_common = u32_at(at + 4)?;
        let returned_file = u32_at(at + 16)?;
        let mut o = at + 24;
        // `attrreference_t`: an offset from the reference itself, and the
        // name's length in bytes with its NUL.
        let name_ref = o;
        let name_offset = i32_at(o)?;
        let name_length = u32_at(o + 4)? as usize;
        o += 8;
        let kind = u32_at(o)?;
        o += 4;
        let mtime = (i64_at(o)?, i64_at(o + 8)?);
        o += 16;
        let ctime = (i64_at(o)?, i64_at(o + 8)?);
        o += 16;
        let file_id = u64::try_from(i64_at(o)?).ok()?;
        o += 8;
        let link_count = u32_at(o)?;
        o += 4;
        let data_length = i64_at(o)?;
        let name_start = usize::try_from(i64::try_from(name_ref).ok()? + i64::from(name_offset))
            .ok()?
            .checked_sub(at)?;
        let name_bytes = record.get(name_start..name_start + name_length)?;
        let name_bytes = name_bytes.strip_suffix(&[0]).unwrap_or(name_bytes);
        Some(Self {
            length,
            returned_common,
            returned_file,
            name: String::from_utf8_lossy(name_bytes).into_owned(),
            kind,
            mtime,
            ctime,
            file_id,
            link_count,
            data_length,
        })
    }

    /// A regular file's stat key — [`StatKey::of`]'s from `lstat` —
    /// `Some(None)` for any other object, and `None` when an attribute the
    /// answer needs did not come back.
    fn stat(&self) -> Option<Option<StatKey>> {
        if self.returned_common & libc::ATTR_CMN_OBJTYPE == 0 {
            return None;
        }
        if self.kind != VREG {
            return Some(None);
        }
        let common = libc::ATTR_CMN_MODTIME | libc::ATTR_CMN_CHGTIME | libc::ATTR_CMN_FILEID;
        let file = libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_DATALENGTH;
        if self.returned_common & common != common || self.returned_file & file != file {
            return None;
        }
        let ns = |(sec, nsec): (i64, i64)| i128::from(sec) * 1_000_000_000 + i128::from(nsec);
        Some(Some(StatKey {
            mtime_ns: ns(self.mtime),
            size: u64::try_from(self.data_length).ok()?,
            inode: self.file_id,
            ctime_ns: ns(self.ctime),
            nlink: u64::from(self.link_count),
        }))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn sorted(mut v: Vec<(ScanEntry, StatKey)>) -> Vec<(ScanEntry, StatKey)> {
        v.sort_by(|a, b| a.0.file_name.cmp(&b.0.file_name));
        v
    }

    /// Owner decision LQ16: the bulk listing answers exactly what `readdir`
    /// and `lstat` answer — the same entries, and for each the same size,
    /// modification time to the nanosecond (before 1970 too), change time,
    /// inode and link count — over every kind of entry `notes/` can hold:
    /// notes, an empty one, a hard-linked one, a link, a directory, hidden
    /// and temp names, other extensions, a name outside ASCII, and more
    /// entries than one batch holds.
    #[test]
    fn the_bulk_listing_is_the_lstat_listing_bit_for_bit() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let id = |n: usize| format!("{:08x}-0000-4000-8000-{:012x}", n, n);
        for n in 0..3000 {
            fs::write(
                d.join(format!("{}--note-{n}.md", id(n))),
                "x".repeat(n % 97),
            )
            .unwrap();
        }
        fs::write(d.join(format!("{}--empty.md", id(90_001))), "").unwrap();
        let nanos = d.join(format!("{}--nanos.md", id(90_002)));
        fs::write(&nanos, "body").unwrap();
        File::options()
            .write(true)
            .open(&nanos)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::new(1_758_000_000, 123_456_789))
            .unwrap();
        let old = d.join(format!("{}--old.md", id(90_003)));
        fs::write(&old, "old").unwrap();
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(UNIX_EPOCH - Duration::new(86_400, 0) + Duration::new(0, 250_000_000))
            .unwrap();
        fs::hard_link(&nanos, d.join(format!("{}--linked.md", id(90_004)))).unwrap();
        std::os::unix::fs::symlink(&nanos, d.join(format!("{}--symlink.md", id(90_005)))).unwrap();
        fs::create_dir(d.join(format!("{}--dir.md", id(90_006)))).unwrap();
        fs::write(d.join(".tmp-write.md"), "temp").unwrap();
        fs::write(d.join(".hidden.md"), "hidden").unwrap();
        fs::write(d.join("notes.txt"), "text").unwrap();
        fs::write(d.join("MEMORY.md.bak"), "backup").unwrap();
        fs::write(d.join("café-日本語 note.md"), "unicode").unwrap();
        fs::write(d.join("no-id.md"), "no id").unwrap();

        let reference = sorted(list_by_lstat(d).unwrap().unwrap());
        let bulk = sorted(list_bulk(d).unwrap().unwrap());
        assert_eq!(reference.len(), 3000 + 6, "the notes, and no other entry");
        assert_eq!(bulk, reference);
        let find = |name: &str| {
            bulk.iter()
                .find(|(e, _)| e.file_name.contains(name))
                .unwrap_or_else(|| panic!("{name}"))
        };
        assert_eq!(find("--linked.md").1.nlink, 2);
        assert_eq!(find("--nanos.md").1.mtime_ns % 1_000_000_000, 123_456_789);
        assert!(find("--old.md").0.mtime.unwrap() < SystemTime::UNIX_EPOCH);
        assert!(find("café").0.id.is_none());

        assert!(list_bulk(&d.join("absent")).unwrap().is_none());
        assert!(list_by_lstat(&d.join("absent")).unwrap().is_none());
    }
}
