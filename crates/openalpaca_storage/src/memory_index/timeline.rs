//! The timeline (design 09 §13.1, MD-14): the notes around one note, or
//! around an instant, in the order of their dates — `observed_at`, else
//! `created_at`. Every status a note can leave recall by is listed (and
//! reported, for the caller to mark): superseded, archived and ended-window
//! notes; a candidate never is, and neither is a note of a partition the
//! caller did not name.

use chrono::{DateTime, Utc};
use rusqlite::types::Value;
use rusqlite::{OptionalExtension, params_from_iter};

use super::{
    EffectiveStatus, IndexError, IndexState, MemoryIndex, Result, canon_timestamp, read_state,
};

/// What a timeline is drawn around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelinePoint {
    /// A note of one of the named partitions: the notes dated before it and
    /// after it, itself apart. Notes of one date are ordered by id, so every
    /// note has exactly one place.
    Note { partition_key: String, id: String },
    /// An instant: `before` notes dated earlier, `after` notes dated at it
    /// or later.
    Instant(DateTime<Utc>),
}

/// One timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineQuery {
    /// The partitions the caller resolved and authorized. Empty means
    /// nothing is listed.
    pub partition_keys: Vec<String>,
    pub point: TimelinePoint,
    /// How many notes before the point, the nearest ones.
    pub before: usize,
    /// How many notes after the point, the nearest ones.
    pub after: usize,
}

/// One listed note: its catalog row and its date, no body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineRow {
    pub id: String,
    pub partition_key: String,
    pub scope: String,
    pub revision: String,
    pub relative_path: String,
    pub title: String,
    pub kind: String,
    pub effective_status: EffectiveStatus,
    pub origin: String,
    /// `observed_at`, else `created_at`, in the canon.
    pub date: String,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    /// The window ended by the index clock's "now".
    pub ended: bool,
    pub conflict: bool,
    pub same_key_loser: bool,
    pub source_forgotten: bool,
}

/// What [`MemoryIndex::timeline`] found. Both sides are oldest first, so
/// `before`, the anchor, then `after` read in time order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    /// The anchor note's own row — `None` for an instant, and for a note
    /// the index does not list (unknown, of another partition, or a
    /// candidate), in which case nothing else is listed either.
    pub anchor: Option<TimelineRow>,
    pub before: Vec<TimelineRow>,
    pub after: Vec<TimelineRow>,
    pub index_state: IndexState,
}

const COLUMNS: &str = "n.id, n.partition_key, n.scope, n.revision, n.relative_path, n.title, \
     n.kind, n.effective_status, n.origin, n.date, n.valid_from, n.valid_to, \
     n.valid_to IS NOT NULL AND n.valid_to <= ?, n.conflict, n.same_key_loser, \
     n.source_forgotten";

/// Everything but a candidate (design 09 §13.1).
const LISTED: &str = "n.effective_status IN ('active', 'superseded', 'archived')";

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<TimelineRow> {
    let status: String = r.get(7)?;
    Ok(TimelineRow {
        id: r.get(0)?,
        partition_key: r.get(1)?,
        scope: r.get(2)?,
        revision: r.get(3)?,
        relative_path: r.get(4)?,
        title: r.get(5)?,
        kind: r.get(6)?,
        effective_status: EffectiveStatus::parse(&status).unwrap_or(EffectiveStatus::Active),
        origin: r.get(8)?,
        date: r.get(9)?,
        valid_from: r.get(10)?,
        valid_to: r.get(11)?,
        ended: r.get(12)?,
        conflict: r.get::<_, i64>(13)? != 0,
        same_key_loser: r.get::<_, i64>(14)? != 0,
        source_forgotten: r.get::<_, i64>(15)? != 0,
    })
}

fn in_list(n: usize) -> String {
    vec!["?"; n].join(", ")
}

impl MemoryIndex {
    /// The notes of the named partitions around `q.point`, the nearest
    /// `q.before` dated before it and the nearest `q.after` dated after it,
    /// each side oldest first. Candidates are never listed; superseded,
    /// archived and ended-window notes are, with what marks them.
    pub fn timeline(&self, q: &TimelineQuery) -> Result<Timeline> {
        let now = canon_timestamp(&self.now());
        let conn = self.lock();
        let index_state = read_state(&conn)?;
        let empty = |index_state| Timeline {
            anchor: None,
            before: Vec::new(),
            after: Vec::new(),
            index_state,
        };
        if q.partition_keys.is_empty() {
            return Ok(empty(index_state));
        }
        let partitions = in_list(q.partition_keys.len());
        let keys = q.partition_keys.iter().map(|k| Value::Text(k.clone()));

        // The point as an order key: `(date, id)` for a note, whose own row
        // is then neither before nor after it; `(instant, "")` for an
        // instant, which puts a note dated exactly then after it.
        let (anchor, date, id) = match &q.point {
            TimelinePoint::Note { partition_key, id } => {
                if !q.partition_keys.contains(partition_key) {
                    return Ok(empty(index_state));
                }
                let sql = format!(
                    "SELECT {COLUMNS} FROM note n
                     WHERE n.partition_key = ? AND n.id = ? AND {LISTED}"
                );
                let params = [
                    Value::Text(now.clone()),
                    Value::Text(partition_key.clone()),
                    Value::Text(id.clone()),
                ];
                let row = conn
                    .query_row(&sql, params_from_iter(params.iter()), row_from)
                    .optional()?;
                let Some(row) = row else {
                    return Ok(empty(index_state));
                };
                let (date, id) = (row.date.clone(), row.id.clone());
                (Some(row), date, id)
            }
            TimelinePoint::Instant(at) => (None, canon_timestamp(at), String::new()),
        };

        let side = |earlier: bool, limit: usize| -> Result<Vec<TimelineRow>> {
            if limit == 0 {
                return Ok(Vec::new());
            }
            let (cmp, order) = if earlier {
                ("n.date < ? OR (n.date = ? AND n.id < ?)", "DESC")
            } else {
                ("n.date > ? OR (n.date = ? AND n.id > ?)", "ASC")
            };
            let sql = format!(
                "SELECT {COLUMNS} FROM note n
                 WHERE n.partition_key IN ({partitions}) AND {LISTED} AND ({cmp})
                 ORDER BY n.date {order}, n.id {order}
                 LIMIT ?"
            );
            let mut params: Vec<Value> = vec![Value::Text(now.clone())];
            params.extend(keys.clone());
            params.push(Value::Text(date.clone()));
            params.push(Value::Text(date.clone()));
            params.push(Value::Text(id.clone()));
            params.push(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
            let mut stmt = conn.prepare_cached(&sql)?;
            let rows = stmt.query_map(params_from_iter(params.iter()), row_from)?;
            let mut rows = rows
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(IndexError::from)?;
            if earlier {
                rows.reverse();
            }
            Ok(rows)
        };
        let before = side(true, q.before)?;
        let after = side(false, q.after)?;
        Ok(Timeline {
            anchor,
            before,
            after,
            index_state,
        })
    }
}
