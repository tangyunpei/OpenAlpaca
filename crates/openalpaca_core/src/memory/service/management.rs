//! The management reads and the owner-wide reconcile behind
//! `/v1/memories/{id}/history`, `?revision=`, `/v1/memories/doctor` and the
//! daemon's reconciliation timer (design §5 *Freshness*, §6 *Management
//! operations*; plan 3.8).

use super::{
    Freshness, HistoryEntry, MemoryAccessContext, MemoryChange, MemoryError, MemoryService,
    READ_MAX_BYTES, ReadOutput, SyncReport, cut_bytes, format_ts, scope_of, source_json,
};
use chrono::{DateTime, Utc};
use openalpaca_storage::memory_files::{
    self as files, DoctorReport, NoteId, Origin, PartitionHealth, revision_of,
};
use openalpaca_storage::store::StoreScope;
use openalpaca_storage::store::projects::ProjectId;

impl MemoryService {
    /// The service-written revisions kept for `id`, newest first (design
    /// §2: the last 5, the current one included). The note must be one of
    /// the owner's.
    pub fn history(
        &self,
        ctx: &MemoryAccessContext,
        id: NoteId,
    ) -> Result<Vec<HistoryEntry>, MemoryError> {
        Self::require(ctx.capabilities.read, "memory_read")?;
        let resolved = self.resolve(ctx)?;
        let Some((p, _)) = Self::find_note(&resolved, &id)? else {
            return Err(MemoryError::NotFound(id));
        };
        Ok(p.history(&id)?
            .into_iter()
            .map(|(revision, written_at)| HistoryEntry {
                revision,
                written_at: written_at.map(DateTime::<Utc>::from),
            })
            .collect())
    }

    /// One kept revision's own bytes as a `memory_read` result: the old
    /// frontmatter and body, beside the note's **current** facts — origin,
    /// links, conflict — which belong to the id, not to the bytes. The
    /// copy is `verified` when its bytes hash to the name it is kept under.
    pub fn read_revision(
        &self,
        ctx: &MemoryAccessContext,
        id: NoteId,
        revision: &str,
    ) -> Result<ReadOutput, MemoryError> {
        Self::require(ctx.capabilities.read, "memory_read")?;
        if !files::is_revision(revision) {
            return Err(MemoryError::invalid(
                "revision is not a 64-character hex revision",
            ));
        }
        let resolved = self.resolve(ctx)?;
        let Some((p, view)) = Self::find_note(&resolved, &id)? else {
            return Err(MemoryError::NotFound(id));
        };
        let Some(bytes) = p.history_bytes(&id, revision)? else {
            return Err(MemoryError::RevisionUnavailable {
                id,
                revision: revision.to_string(),
            });
        };
        let note = files::Note::parse(&bytes).map_err(|e| {
            MemoryError::Internal(anyhow::anyhow!(
                "kept revision {revision} of note {id} does not parse: {}",
                e.reason
            ))
        })?;
        let freshness = if revision_of(&bytes) == revision {
            Freshness::Verified
        } else {
            Freshness::Unverified
        };
        let (body, truncated) = cut_bytes(&note.body, READ_MAX_BYTES);
        Ok(ReadOutput {
            id: note.id,
            revision: revision.to_string(),
            title: note.title.clone(),
            kind: note.kind,
            scope: scope_of(p),
            status: note.status,
            effective_status: view.derived.effective_status,
            origin: view.derived.origin,
            frontmatter: frontmatter_json(&note),
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

    /// The read-only doctor (design §6, *Management operations*) under this
    /// context: the owner decides what an orphan is, the resolved project
    /// root is scanned beside the home store. Changes nothing
    /// ([`files::doctor`] opens every partition read-only).
    pub fn doctor(&self, ctx: &MemoryAccessContext) -> Result<DoctorReport, MemoryError> {
        Self::require(ctx.capabilities.read, "memory_read")?;
        Ok(files::doctor(&ctx.owner_id, ctx.project_root.as_deref())?)
    }

    /// ST-19 step 3 (design §6, *Delete its data*): every owner's partition
    /// of the project — a control snapshot carrying its id — is dropped from
    /// the index, forgotten by this process's fingerprints, scans and parse
    /// cache, and its snapshot removed from `state/memory-control/`. The note
    /// files go with `projects/<name>/` (`project_life::remove_project_folder`).
    ///
    /// The caller holds [`MemoryService::quiesce`] for the project; this
    /// takes no write slot, so it never waits on that hold. Answers how many
    /// partitions were dropped. An unreadable snapshot is skipped: it cannot
    /// be told to be this project's.
    pub fn remove_project(&self, project_id: &ProjectId) -> Result<usize, MemoryError> {
        let snapshots = files::list_snapshots()?;
        let mut fingerprints = self.fingerprints.lock().unwrap_or_else(|e| e.into_inner());
        let mut last_scans = self.last_scans.lock().unwrap_or_else(|e| e.into_inner());
        let mut removed = 0;
        for (path, parsed) in snapshots {
            let Ok(snapshot) = parsed else {
                continue;
            };
            if snapshot.scope != files::SnapshotScope::Project
                || snapshot.project_id.as_ref() != Some(project_id)
            {
                continue;
            }
            let key = snapshot.partition_key;
            self.indexed(self.index.remove_partition(&key))?;
            fingerprints.remove(&key);
            last_scans.remove(&key);
            self.parse_cache.evict_partition(&key);
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(MemoryError::Internal(anyhow::Error::new(e).context(
                        format!("could not remove the control snapshot {}", path.display()),
                    )));
                }
            }
            removed += 1;
        }
        Ok(removed)
    }

    /// The daemon's reconciliation timer (design §5, *Freshness without a
    /// watcher*: a bounded stat scan every 30 s): every partition of
    /// `owner_id` — the home one and one in each registered project that
    /// holds its directory ([`Self::known_project_roots`], design §5) — is
    /// re-hashed, a change made behind the service's back is re-recorded
    /// `external` and published as `memory_changed` with no surface (§7b
    /// P13), and the index follows.
    /// Needs no context: nothing here is on a caller's behalf. A partition
    /// that is quarantined, absent or refused (a link on its path) is
    /// skipped, and so — with one WARN — is one whose open or scan fails
    /// for any other reason: one broken partition never stops the tick for
    /// the partitions after it.
    ///
    /// A whole rebuild the index is owed — left by the daemon's start, a
    /// failed attempt, or damage met on use — runs first, here, off the
    /// request path (scale slice S6); the index's idle upkeep runs last
    /// (scale slice S5).
    pub fn reconcile_owner(&self, owner_id: &str) -> Result<SyncReport, MemoryError> {
        if let Err(e) = self.run_pending_rebuild() {
            tracing::warn!(
                "memory: the search index could not be rebuilt; recall and search read the \
                 note files meanwhile, and the next tick tries again: {e}"
            );
        }
        let owner_key = files::owner_key(owner_id);
        let mut scopes = vec![StoreScope::Home];
        scopes.extend(
            self.known_project_roots(owner_id)
                .into_iter()
                .map(StoreScope::Project),
        );
        let mut scanned = 0;
        let mut changes = Vec::new();
        for scope in &scopes {
            let _write = self.slots.begin_write(&Self::slot_key(scope, &owner_key));
            let partition = match self.open_cached(scope, owner_id) {
                Ok(p) => p,
                Err(files::MemoryFilesError::PathRefused { component, reason }) => {
                    tracing::debug!(
                        component = %component.display(),
                        %reason,
                        "memory: partition skipped by the reconciliation scan"
                    );
                    continue;
                }
                Err(e) => {
                    tracing::warn!(
                        scope = ?scope,
                        "memory: partition skipped by the reconciliation scan, it did not open: {e}"
                    );
                    continue;
                }
            };
            if partition.health() != PartitionHealth::Ready {
                continue;
            }
            match self.reconcile_partition(&partition) {
                Ok((count, found)) => {
                    scanned += count;
                    changes.extend(found);
                }
                Err(e) => {
                    self.note_index_failure(&e);
                    tracing::warn!(
                        partition = partition.partition_key(),
                        "memory: partition skipped by the reconciliation scan: {e}"
                    );
                }
            }
        }
        // Upkeep once the index has sat idle for a tick (scale slice S5).
        self.maintain_index_when_idle();
        Ok(SyncReport {
            scanned,
            changes,
            index_state: self.current_index_state()?,
        })
    }

    /// One tick's work on one ready partition, under its write slot: the
    /// stat scan and — only when it differs from the last tick's, or on the
    /// first tick — the re-hash of what changed behind the service's back
    /// (published as it is found) and the index's catch-up. A scan that
    /// matches the last tick's costs the `read_dir` and nothing else; the
    /// scan is remembered once its work has succeeded, so a tick that
    /// failed halfway is redone. Answers how many files the scan listed
    /// and the external changes it recorded.
    fn reconcile_partition(
        &self,
        partition: &files::Partition,
    ) -> Result<(usize, Vec<MemoryChange>), MemoryError> {
        // The scan leaves out, and deletes, a forgotten note's file that
        // came back; a forgotten note's `.history` put back on its own is
        // met by no scan, so each tick deletes it here (scale slice S3).
        let mut scan = partition.scan()?;
        scan.sort_by(|a, b| a.file_name.cmp(&b.file_name));
        let scanned = scan.len();
        let key = partition.partition_key();
        if let Err(e) = partition.sweep_forgotten_history() {
            tracing::warn!(
                partition = key,
                "memory: a forgotten note's history could not be deleted: {e}"
            );
        }
        let unchanged = {
            let last = self.last_scans.lock().unwrap_or_else(|e| e.into_inner());
            last.get(key) == Some(&scan)
        };
        if unchanged {
            return Ok((scanned, Vec::new()));
        }
        let mut changes = Vec::new();
        let (found, view) = partition.reconcile_viewed()?;
        for change in found {
            let change = MemoryChange {
                partition_key: key.to_string(),
                note_id: change.id,
                change: "external",
                revision: change.revision,
                origin: Origin::External,
                surface: None,
            };
            self.publish(&change);
            changes.push(change);
        }
        // A pass a running rebuild skipped did not bring the rows in line
        // with this scan: the next tick looks again (the L8 product slice's
        // P6), as it does after the rebuild, which forgets every last scan.
        if self.reindex_view_unless_rebuilding(partition, &view)? {
            self.last_scans
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key.to_string(), scan);
        }
        Ok((scanned, changes))
    }
}

/// A note's frontmatter as `memory_read` serves it (design §7a).
pub(super) fn frontmatter_json(note: &files::Note) -> serde_json::Value {
    serde_json::json!({
        "schema": files::NOTE_SCHEMA,
        "id": note.id.to_string(),
        "title": note.title,
        "kind": note.kind.as_str(),
        "status": note.status.as_str(),
        "fact_key": note.fact_key,
        "aliases": note.aliases,
        "tags": note.tags,
        "created_at": format_ts(note.created_at),
        "updated_at": format_ts(note.updated_at),
        "observed_at": note.observed_at.map(format_ts),
        "valid_from": note.valid_from.map(format_ts),
        "valid_to": note.valid_to.map(format_ts),
        "review_after": note.review_after.map(format_ts),
        "pinned": note.pinned,
        "sources": note.sources.iter().map(source_json).collect::<Vec<_>>(),
        "supersedes": note.supersedes.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
    })
}
