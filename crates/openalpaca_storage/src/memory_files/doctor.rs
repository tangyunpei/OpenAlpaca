//! `memory doctor`, the read-only scan (design §6, *Management operations*;
//! plan 3.8): what `GET /v1/memories/doctor` answers and what
//! `openalpaca memory doctor` prints when no daemon runs.
//!
//! It lists, per partition, what the design names — quarantine (a snapshot
//! missing or invalid), orphans (an owner key that is not the current
//! owner's), conflicts (a lower-authority `supersedes` link, two active
//! same-`fact_key` notes), unverified and malformed files — and the stale,
//! abandoned and orphaned move snapshots of §6's workspace-move row. It
//! changes nothing: every partition is opened with
//! [`Partition::open_readonly`], which re-asserts no `.gitignore` and runs no
//! recovery, and the snapshots are read where they lie.

use super::authority::Conflict;
use super::partition::{
    MalformedFile, MoveSnapshotState, Partition, PartitionHealth, classify_move_snapshots,
    list_owner_keys, list_snapshots,
};
use super::snapshot::{ControlSnapshot, SnapshotScope};
use super::{MemoryFilesError, NoteId, owner_key};
use crate::store::StoreScope;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One partition as the doctor found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionReport {
    pub partition_key: String,
    pub scope: SnapshotScope,
    pub owner_key: String,
    /// The owner key is not the current owner's (design §3, *Resets*):
    /// `openalpaca memory adopt <owner-key>` is the verb for it.
    pub orphan: bool,
    /// The partition directory (`…/memory/owners/<owner-key>`).
    pub root: PathBuf,
    pub health: PartitionHealth,
    /// Notes that could be served (parsed, one file per id).
    pub notes: usize,
    /// Notes the snapshot does not record at their current revision — every
    /// note of a quarantined partition, and any file edited behind the
    /// service's back since the last scan.
    pub unverified: usize,
    pub malformed: Vec<MalformedFile>,
    /// Ids two or more files claim; none of those files is served.
    pub duplicates: Vec<(NoteId, Vec<PathBuf>)>,
    pub conflicts: Vec<Conflict>,
    pub tombstones: usize,
    /// A write the snapshot still names as in flight (design §6): recovery
    /// finishes it at the next ordinary open.
    pub pending: Option<NoteId>,
}

/// A snapshot whose bound directory does not exist (design §6, the
/// workspace-move row; §7b P11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveSnapshotReport {
    pub partition_key: String,
    pub state: MoveSnapshotState,
    /// The directory the snapshot is bound to, as recorded.
    pub root: String,
    pub moved_from: Option<String>,
}

/// A partition the doctor could not open: a symlink, a junction or a
/// non-directory on the way to it (design §3 rule 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedPartition {
    pub scope: SnapshotScope,
    pub owner_key: String,
    pub component: PathBuf,
    pub reason: String,
}

/// The whole read-only report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    /// The current owner's key, which decides `orphan`.
    pub owner_key: String,
    pub partitions: Vec<PartitionReport>,
    pub move_snapshots: Vec<MoveSnapshotReport>,
    pub refused: Vec<RefusedPartition>,
    /// Snapshot files under `state/memory-control/` that do not parse, with
    /// why; the partition each belongs to is quarantined wherever its
    /// directory is.
    pub unreadable_snapshots: Vec<(PathBuf, String)>,
}

/// The scan. `current_owner_id` decides which partitions are orphans;
/// `project` is the request's project root (the marker root, design §3 rule
/// 1), whose owner directories are scanned beside the home store's. Every
/// project snapshot whose directory exists is scanned too, whichever project
/// it belongs to, so a partition is never missed because the caller stood
/// elsewhere. The current owner's home partition is reported even when it
/// does not exist yet (`Absent`).
pub fn doctor(
    current_owner_id: &str,
    project: Option<&Path>,
) -> Result<DoctorReport, MemoryFilesError> {
    let current_key = owner_key(current_owner_id);
    let mut report = DoctorReport {
        owner_key: current_key.clone(),
        partitions: Vec::new(),
        move_snapshots: Vec::new(),
        refused: Vec::new(),
        unreadable_snapshots: Vec::new(),
    };

    let mut snapshots: Vec<ControlSnapshot> = Vec::new();
    for (path, parsed) in list_snapshots()? {
        match parsed {
            Ok(s) => snapshots.push(s),
            Err(reason) => report.unreadable_snapshots.push((path, reason)),
        }
    }
    for (partition_key, state) in classify_move_snapshots(&snapshots) {
        let Some(s) = snapshots.iter().find(|s| s.partition_key == partition_key) else {
            continue;
        };
        report.move_snapshots.push(MoveSnapshotReport {
            partition_key,
            state,
            root: s.root.clone(),
            moved_from: s.moved_from.clone(),
        });
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut scopes = vec![StoreScope::Home];
    if let Some(project) = project {
        scopes.push(StoreScope::Project(project.to_path_buf()));
    }
    for scope in &scopes {
        let mut keys = list_owner_keys(scope)?;
        if !keys.contains(&current_key) {
            keys.push(current_key.clone());
        }
        keys.sort();
        for key in keys {
            scan_partition(&mut report, &mut seen, scope, &key, &current_key);
        }
    }
    // Project partitions the snapshots know that the caller did not name.
    for s in snapshots
        .iter()
        .filter(|s| s.scope == SnapshotScope::Project && Path::new(&s.root).is_dir())
    {
        if seen.contains(&s.partition_key) {
            continue;
        }
        let Some(project) = Path::new(&s.root).ancestors().nth(4) else {
            continue;
        };
        scan_partition(
            &mut report,
            &mut seen,
            &StoreScope::Project(project.to_path_buf()),
            &s.owner_key,
            &current_key,
        );
    }
    Ok(report)
}

fn scan_partition(
    report: &mut DoctorReport,
    seen: &mut BTreeSet<String>,
    scope: &StoreScope,
    key: &str,
    current_key: &str,
) {
    let scope_kind = match scope {
        StoreScope::Home => SnapshotScope::Home,
        StoreScope::Project(_) => SnapshotScope::Project,
    };
    let partition = match Partition::open_readonly(scope, key) {
        Ok(p) => p,
        Err(MemoryFilesError::PathRefused { component, reason }) => {
            report.refused.push(RefusedPartition {
                scope: scope_kind,
                owner_key: key.to_string(),
                component,
                reason,
            });
            return;
        }
        Err(e) => {
            report.refused.push(RefusedPartition {
                scope: scope_kind,
                owner_key: key.to_string(),
                component: PathBuf::new(),
                reason: e.to_string(),
            });
            return;
        }
    };
    if !seen.insert(partition.partition_key().to_string()) {
        return;
    }
    let loaded = match partition.load() {
        Ok(l) => l,
        Err(e) => {
            report.refused.push(RefusedPartition {
                scope: scope_kind,
                owner_key: key.to_string(),
                component: partition.root().to_path_buf(),
                reason: e.to_string(),
            });
            return;
        }
    };
    let snapshot = partition.snapshot();
    report.partitions.push(PartitionReport {
        partition_key: partition.partition_key().to_string(),
        scope: scope_kind,
        owner_key: key.to_string(),
        orphan: key != current_key,
        root: partition.root().to_path_buf(),
        health: loaded.health,
        notes: loaded.views.len(),
        unverified: loaded
            .views
            .iter()
            .filter(|v| v.unverified || !v.derived.recorded)
            .count(),
        malformed: loaded.malformed,
        duplicates: loaded.duplicates,
        conflicts: loaded.conflicts,
        tombstones: snapshot.as_ref().map_or(0, |s| s.tombstones.len()),
        pending: snapshot.and_then(|s| s.pending.map(|p| p.id)),
    });
}
