//! Tests for the memory index: the chunker, the query builder, the catalog,
//! search semantics, the stat scan, rebuild equivalence and `index_state`.
//! Temp directories only; no store root, no daemon, no network.

use super::*;
use tempfile::tempdir;

// ============================================================================
// Open and state
// ============================================================================

#[test]
fn a_fresh_index_reports_rebuilding_until_rebuilt() {
    let index = MemoryIndex::open_in_memory().unwrap();
    let report = index.index_state().unwrap();
    assert_eq!(report.state, IndexState::Rebuilding);
    assert_eq!(report.schema_version, SCHEMA_VERSION);
    assert_eq!(report.notes, 0);
    assert_eq!(report.chunks, 0);
    assert_eq!(report.partitions, 0);
    assert_eq!(report.path, None);
}

#[test]
fn open_creates_the_file_and_its_parent_directory() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("state").join("cache").join(INDEX_FILE_NAME);
    let index = MemoryIndex::open(&path).unwrap();
    assert!(path.exists());
    assert_eq!(index.path(), Some(path.as_path()));
    assert_eq!(index.index_state().unwrap().state, IndexState::Rebuilding);
}

#[test]
fn open_recreates_a_corrupt_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    std::fs::write(&path, b"this is not a database").unwrap();
    let index = MemoryIndex::open(&path).unwrap();
    assert_eq!(index.index_state().unwrap().state, IndexState::Rebuilding);
    assert!(path.exists());
}

#[test]
fn open_recreates_a_file_of_another_schema_version() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    {
        let index = MemoryIndex::open(&path).unwrap();
        let conn = index.lock();
        conn.execute(
            "UPDATE meta SET value = '999' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        write_state(&conn, IndexState::Ready).unwrap();
    }
    let index = MemoryIndex::open(&path).unwrap();
    let report = index.index_state().unwrap();
    assert_eq!(report.state, IndexState::Rebuilding);
    assert_eq!(report.schema_version, SCHEMA_VERSION);
}

#[test]
fn canonicalize_timestamp_normalizes_offsets_and_subseconds() {
    assert_eq!(
        canonicalize_timestamp("t", "2026-06-01T02:00:00.123+02:00").unwrap(),
        "2026-06-01T00:00:00Z"
    );
    assert_eq!(
        canonicalize_timestamp("t", "2026-06-01T00:00:00Z").unwrap(),
        "2026-06-01T00:00:00Z"
    );
    let err = canonicalize_timestamp("valid_to", "June 2026").unwrap_err();
    assert!(matches!(err, IndexError::InvalidInput(m) if m.contains("valid_to")));
}
