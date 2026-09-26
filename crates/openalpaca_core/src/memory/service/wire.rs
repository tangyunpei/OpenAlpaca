//! The §7a JSON of every service result — one renderer for the six tools
//! (`tools/builtins/memory_tools.rs`) and the `/v1/memories*` routes, so a
//! result reads the same on the tool surface and on the wire.

use super::{
    ForgetItemStatus, ForgetOutput, HistoryEntry, IndexRow, ListItem, ListOutput, ReadManyItem,
    ReadManyOutput, ReadOutput, RecentlyUsedItem, SearchDetail, SearchOutput, StoreOutput,
    Suggestion, SyncReport, TimelineOutput, UpdateOutput, WithdrawOutput, format_ts, source_json,
};
use openalpaca_storage::memory_files::{
    Dedup, DoctorReport, NoteId, PartitionHealth, SnapshotScope,
};
use serde_json::{Value, json};

pub fn ids_json(ids: &[NoteId]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

fn sources_status(source_forgotten: bool) -> &'static str {
    if source_forgotten {
        "source_forgotten"
    } else {
        "intact"
    }
}

/// `memory_search`'s result in the detail asked for (design 09 §13.1): index
/// rows, or the cited snippets of [`search_output_json`].
pub fn search_json(out: &SearchOutput, detail: SearchDetail) -> Value {
    match detail {
        SearchDetail::Index => search_index_json(out),
        SearchDetail::Snippets => search_output_json(out),
    }
}

/// One index row (design 09 §13.1): `{id, date, kind, scope, status, title,
/// reason, size}` — no body, no excerpt — and §7b P3's `conflict`, which a
/// search result has always carried.
pub fn index_row_json(row: &IndexRow) -> Value {
    json!({
        "id": row.id.to_string(),
        "date": format_ts(row.date),
        "kind": row.kind.as_str(),
        "scope": row.scope.as_str(),
        "status": row.status.as_str(),
        "title": row.title,
        "reason": row.reason,
        "size": row.size,
        "conflict": row.conflict,
    })
}

/// `memory_search`'s index (design 09 §13.1): one row per hit, in the
/// search's order, with the same `count`, `partial` and `index_state` as the
/// snippets.
pub fn search_index_json(out: &SearchOutput) -> Value {
    let results: Vec<Value> = out
        .results
        .iter()
        .map(|r| index_row_json(&r.index_row()))
        .collect();
    let count = results.len();
    json!({
        "results": results,
        "count": count,
        "partial": out.partial,
        "index_state": out.index_state.as_str(),
    })
}

/// `memory_timeline`'s result (design 09 §13.1): the rows in time order,
/// each `reason` saying `before`, `anchor` or `after`, and what the
/// timeline was drawn around — `{id}` of the anchor note, `{date}` of an
/// instant, or null when a query matched nothing.
pub fn timeline_json(out: &TimelineOutput) -> Value {
    let anchor = match (out.anchor_note, out.anchor_date) {
        (Some(id), _) => json!({ "id": id.to_string() }),
        (None, Some(at)) => json!({ "date": format_ts(at) }),
        (None, None) => Value::Null,
    };
    json!({
        "anchor": anchor,
        "results": out.rows.iter().map(index_row_json).collect::<Vec<_>>(),
        "count": out.rows.len(),
        "index_state": out.index_state.as_str(),
    })
}

/// `memory_read`'s result with its ids (design 09 §13.1): one entry per id,
/// in the order asked — the §7a read object, or `{id, error}` for an id
/// that could not be read (`not_found`; `revision_conflict` with the
/// current revision). `count` is how many were read; the bodies shared
/// `budget_bytes`.
pub fn read_many_json(out: &ReadManyOutput) -> Value {
    let mut count = 0;
    let notes: Vec<Value> = out
        .items
        .iter()
        .map(|item| match item {
            ReadManyItem::Read(read) => {
                count += 1;
                read_output_json(read)
            }
            ReadManyItem::NotFound(id) => json!({ "id": id.to_string(), "error": "not_found" }),
            ReadManyItem::RevisionConflict {
                id,
                current_revision,
            } => json!({
                "id": id.to_string(),
                "error": "revision_conflict",
                "current_revision": current_revision,
            }),
        })
        .collect();
    json!({
        "notes": notes,
        "count": count,
        "budget_bytes": out.budget_bytes,
    })
}

/// `memory_search`'s result (design §7a; §7b P3/P6), with each note's
/// `valid_from` and `valid_to` (RFC 3339 or null; design 09 §6.1).
pub fn search_output_json(out: &SearchOutput) -> Value {
    let results: Vec<Value> = out
        .results
        .iter()
        .map(|r| {
            json!({
                "id": r.id.to_string(),
                "revision": r.revision,
                "title": r.title,
                "kind": r.kind.as_str(),
                "scope": r.scope.as_str(),
                "status": r.status.as_str(),
                "effective_status": r.effective_status.as_str(),
                "origin": r.origin.as_str(),
                "excerpt": r.excerpt,
                "heading": r.heading,
                "line_range": [r.line_range.0, r.line_range.1],
                "match": r.matches.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
                "freshness": r.freshness.as_str(),
                "conflict": r.conflict,
                "same_key_loser": r.same_key_loser,
                "sources": r.sources.iter().map(source_json).collect::<Vec<_>>(),
                "sources_status": sources_status(r.source_forgotten),
                "superseded_by": ids_json(&r.superseded_by),
                "path": r.path,
                "pinned": r.pinned,
                "valid_from": r.valid_from.map(format_ts),
                "valid_to": r.valid_to.map(format_ts),
            })
        })
        .collect();
    let count = results.len();
    json!({
        "results": results,
        "count": count,
        "partial": out.partial,
        "index_state": out.index_state.as_str(),
    })
}

/// `memory_read`'s result (design §7a).
pub fn read_output_json(out: &ReadOutput) -> Value {
    json!({
        "id": out.id.to_string(),
        "revision": out.revision,
        "title": out.title,
        "kind": out.kind.as_str(),
        "scope": out.scope.as_str(),
        "status": out.status.as_str(),
        "effective_status": out.effective_status.as_str(),
        "origin": out.origin.as_str(),
        "frontmatter": out.frontmatter,
        "body": out.body,
        "truncated": out.truncated,
        "path": out.path,
        "sources": out.sources.iter().map(source_json).collect::<Vec<_>>(),
        "sources_status": sources_status(out.source_forgotten),
        "supersedes": ids_json(&out.supersedes),
        "superseded_by": ids_json(&out.superseded_by),
        "freshness": out.freshness.as_str(),
        "conflict": out.conflict,
    })
}

/// One `memory_list` item (design §7a), as the list, `recently-learned`
/// and the GUI read it — with `supersedes` (design 09 §3.2, S8), so a
/// recently-learned row can say what it replaced.
pub fn list_item_json(i: &ListItem) -> Value {
    json!({
        "id": i.id.to_string(),
        "revision": i.revision,
        "title": i.title,
        "kind": i.kind.as_str(),
        "scope": i.scope.as_str(),
        "status": i.status.as_str(),
        "effective_status": i.effective_status.as_str(),
        "origin": i.origin.as_str(),
        "updated_at": format_ts(i.updated_at),
        "created_at": format_ts(i.created_at),
        "pinned": i.pinned,
        "supersedes": ids_json(&i.supersedes),
    })
}

/// `memory_list`'s result (design §7a).
pub fn list_output_json(out: &ListOutput) -> Value {
    json!({
        "items": out.items.iter().map(list_item_json).collect::<Vec<_>>(),
        "next_cursor": out.next_cursor,
    })
}

/// `memory_store`'s result (design §7a), plus `dedup` (§7b P8).
pub fn store_output_json(out: &StoreOutput) -> Value {
    json!({
        "id": out.id.to_string(),
        "revision": out.revision,
        "scope": out.scope.as_str(),
        "origin": out.origin.as_str(),
        "superseded": ids_json(&out.superseded),
        "dedup": match out.dedup {
            Dedup::Created => "created",
            Dedup::Existing => "existing",
        },
    })
}

/// `memory_update`'s result (design §7a), with what a confirmed suggestion
/// retired and dismissed (design 09 §3.5; empty for every other update).
pub fn update_output_json(out: &UpdateOutput) -> Value {
    json!({
        "id": out.id.to_string(),
        "revision": out.revision,
        "status": out.status.as_str(),
        "superseded": ids_json(&out.superseded),
        "dismissed": ids_json(&out.dismissed),
    })
}

/// `POST /v1/memories/{id}/undo` (design 09 §3.3): a withdrawal, not a
/// forget — the answer names what came back, and carries no `retained`,
/// because nothing the note cited was suppressed.
pub fn withdraw_output_json(out: &WithdrawOutput) -> Value {
    json!({
        "id": out.id.to_string(),
        "status": "withdrawn",
        "restored": ids_json(&out.restored),
    })
}

/// `GET /v1/memories/suggestions` (design 09 §3.5).
pub fn suggestions_json(items: &[Suggestion]) -> Value {
    json!({
        "items": items
            .iter()
            .map(|i| json!({
                "id": i.id.to_string(),
                "revision": i.revision,
                "title": i.title,
                "knowledge": i.knowledge,
                "knowledge_left_out": i.knowledge_left_out,
                "reason": i.reason,
                "created_at": format_ts(i.created_at),
                "observed_at": i.observed_at.map(format_ts),
                "origin": i.origin.as_str(),
                "scope": i.scope.as_str(),
                "sources": i.sources.iter().map(source_json).collect::<Vec<_>>(),
                "older_count": i.older_count,
                "target": {
                    "id": i.target.id.to_string(),
                    "revision": i.target.revision,
                    "title": i.target.title,
                    "excerpt": i.target.excerpt,
                    "excerpt_left_out": i.target.excerpt_left_out,
                    "origin": i.target.origin.as_str(),
                    "scope": i.target.scope.as_str(),
                },
            }))
            .collect::<Vec<_>>(),
    })
}

/// `memory_forget`'s result (design §7a).
pub fn forget_output_json(out: &ForgetOutput) -> Value {
    let results: Vec<Value> = out
        .results
        .iter()
        .map(|r| match &r.status {
            ForgetItemStatus::Forgotten => {
                json!({ "id": r.id.to_string(), "status": "forgotten" })
            }
            ForgetItemStatus::RevisionConflict { current_revision } => json!({
                "id": r.id.to_string(),
                "status": "revision_conflict",
                "current_revision": current_revision,
            }),
            ForgetItemStatus::NotFound => {
                json!({ "id": r.id.to_string(), "status": "not_found" })
            }
        })
        .collect();
    json!({
        "results": results,
        "retained": retained_json(out),
    })
}

/// What a forget did not erase (design §6, *Forget contract*).
pub fn retained_json(out: &ForgetOutput) -> Value {
    json!({
        "transcript": out.retained.transcript,
        "session_logs": out.retained.session_logs,
        "note": out.retained.note,
    })
}

/// `GET /v1/memories/{id}/history`.
pub fn history_json(entries: &[HistoryEntry]) -> Value {
    json!({
        "revisions": entries
            .iter()
            .map(|e| json!({
                "revision": e.revision,
                "written_at": e.written_at.map(format_ts),
            }))
            .collect::<Vec<_>>(),
    })
}

/// `GET /v1/memories/recently-used`.
pub fn recently_used_json(items: &[RecentlyUsedItem]) -> Value {
    json!({
        "items": items
            .iter()
            .map(|i| json!({
                "id": i.id.to_string(),
                "revision": i.revision,
                "reason": i.reason.as_str(),
                "delivered_at": format_ts(i.delivered_at),
                "lane_key": i.lane_key,
                "task_id": i.task_id,
            }))
            .collect::<Vec<_>>(),
    })
}

/// `POST /v1/memories/sync`: `{scanned, changed, index_state}` plus the
/// changes themselves.
pub fn sync_report_json(report: &SyncReport) -> Value {
    json!({
        "scanned": report.scanned,
        "changed": report.changes.len(),
        "index_state": report.index_state.as_str(),
        "changes": report
            .changes
            .iter()
            .map(|c| json!({
                "partition_key": c.partition_key,
                "note_id": c.note_id.to_string(),
                "revision": c.revision,
            }))
            .collect::<Vec<_>>(),
    })
}

fn scope_word(scope: SnapshotScope) -> &'static str {
    match scope {
        SnapshotScope::Home => "global",
        SnapshotScope::Project => "project",
    }
}

/// `GET /v1/memories/doctor`, and what `openalpaca memory doctor` prints
/// from a local scan: the same shape from either side.
pub fn doctor_report_json(report: &DoctorReport) -> Value {
    json!({
        "owner_key": report.owner_key,
        "partitions": report
            .partitions
            .iter()
            .map(|p| {
                let (health, reason) = match &p.health {
                    PartitionHealth::Ready => ("ready", None),
                    PartitionHealth::Absent => ("absent", None),
                    PartitionHealth::Quarantined(reason) => ("quarantined", Some(reason.clone())),
                };
                json!({
                    "partition_key": p.partition_key,
                    "scope": scope_word(p.scope),
                    "owner_key": p.owner_key,
                    "orphan": p.orphan,
                    "root": p.root.display().to_string(),
                    "health": health,
                    "quarantine_reason": reason,
                    "notes": p.notes,
                    "unverified": p.unverified,
                    "malformed": p
                        .malformed
                        .iter()
                        .map(|m| json!({ "path": m.path.display().to_string(), "reason": m.reason }))
                        .collect::<Vec<_>>(),
                    "duplicates": p
                        .duplicates
                        .iter()
                        .map(|(id, files)| json!({
                            "id": id.to_string(),
                            "files": files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>(),
                        }))
                        .collect::<Vec<_>>(),
                    "conflicts": p
                        .conflicts
                        .iter()
                        .map(|c| json!({
                            "kind": c.kind.as_str(),
                            "note": c.note.to_string(),
                            "target": c.target.to_string(),
                        }))
                        .collect::<Vec<_>>(),
                    "tombstones": p.tombstones,
                    "pending": p.pending.map(|id| id.to_string()),
                })
            })
            .collect::<Vec<_>>(),
        "move_snapshots": report
            .move_snapshots
            .iter()
            .map(|m| json!({
                "partition_key": m.partition_key,
                "state": match m.state {
                    openalpaca_storage::memory_files::MoveSnapshotState::Stale => "stale",
                    openalpaca_storage::memory_files::MoveSnapshotState::Abandoned => "abandoned",
                    openalpaca_storage::memory_files::MoveSnapshotState::Orphaned => "orphaned",
                },
                "root": m.root,
                "moved_from": m.moved_from,
            }))
            .collect::<Vec<_>>(),
        "refused": report
            .refused
            .iter()
            .map(|r| json!({
                "scope": scope_word(r.scope),
                "owner_key": r.owner_key,
                "component": r.component.display().to_string(),
                "reason": r.reason,
            }))
            .collect::<Vec<_>>(),
        "unreadable_snapshots": report
            .unreadable_snapshots
            .iter()
            .map(|(path, reason)| json!({ "path": path.display().to_string(), "reason": reason }))
            .collect::<Vec<_>>(),
    })
}
