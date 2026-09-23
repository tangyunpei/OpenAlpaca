//! The control snapshot (design §7a): one JSON file per partition under
//! `<home-store>/state/memory-control/<partition-key>.json`, replaced
//! atomically, holding what must not live in editable prose — the binding,
//! per-note revision and origin, the single `pending` field crash recovery
//! reads, and tombstones.
//!
//! Every object is closed: an unknown key makes the snapshot *invalid*, which
//! quarantines the partition (design §2, §7a).

use super::fsx::{create_private_dir, read_if_exists, write_atomic_private};
use super::note::{NoteId, format_ts, parse_ts, to_second};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

/// Who recorded a note (design §4), with the five strictly ordered ranks:
/// `user` > `extracted_user` > `imported` > `external` > `extracted_other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    User,
    ExtractedUser,
    ExtractedOther,
    Imported,
    External,
}

impl Origin {
    pub const ALL: &'static [Origin] = &[
        Origin::User,
        Origin::ExtractedUser,
        Origin::ExtractedOther,
        Origin::Imported,
        Origin::External,
    ];

    /// Higher outranks lower (design §4).
    pub fn rank(self) -> u8 {
        match self {
            Origin::User => 5,
            Origin::ExtractedUser => 4,
            Origin::Imported => 3,
            Origin::External => 2,
            Origin::ExtractedOther => 1,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Origin::User => "user",
            Origin::ExtractedUser => "extracted_user",
            Origin::ExtractedOther => "extracted_other",
            Origin::Imported => "imported",
            Origin::External => "external",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|o| o.as_str() == s)
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `scope` of a snapshot (design §7a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotScope {
    Home,
    Project,
}

/// `notes[<uuid>]`: the revision the service last wrote, its origin, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteRecord {
    pub revision: String,
    pub origin: Origin,
    #[serde(with = "ts")]
    pub written_at: DateTime<Utc>,
}

/// The one field crash recovery reads (design §6, §7a). `from_rev` is null on
/// create.
///
/// **`origin` is not in design §7a's `{id, from_rev, to_rev}`.** Recovery's
/// "file equals `to_rev`: finish step 3" must record the origin step 3 would
/// have recorded, and nothing else on disk holds it (the note's own record
/// still carries the pre-write origin until step 3). It is recorded here so
/// the recovery can finish the write it finds; the report names this as a
/// deviation for the design to absorb.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub id: NoteId,
    pub from_rev: Option<String>,
    pub to_rev: String,
    pub origin: Origin,
}

/// A forgotten note's durable trace (design §6, §7a): its id, its origin and
/// `supersedes` list — so forgetting a replacement does not revive what it
/// replaced — and the source ids extraction must not replay. Never the text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tombstone {
    pub id: NoteId,
    pub origin: Origin,
    pub supersedes: Vec<NoteId>,
    pub suppressed_sources: Vec<String>,
    #[serde(with = "ts")]
    pub forgotten_at: DateTime<Utc>,
}

/// The snapshot (design §7a), field order as written there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlSnapshot {
    pub schema: u32,
    pub partition_key: String,
    pub scope: SnapshotScope,
    pub owner_key: String,
    pub owner_id: String,
    pub root: String,
    #[serde(with = "ts")]
    pub created_at: DateTime<Utc>,
    #[serde(with = "ts")]
    pub updated_at: DateTime<Utc>,
    pub moved_from: Option<String>,
    #[serde(with = "ts_opt")]
    pub repaired_at: Option<DateTime<Utc>>,
    pub pending: Option<Pending>,
    pub notes: BTreeMap<NoteId, NoteRecord>,
    pub tombstones: Vec<Tombstone>,
}

/// The snapshot schema this build reads and writes.
pub const SNAPSHOT_SCHEMA: u32 = 1;

/// Why a snapshot file is not a snapshot.
#[derive(Debug)]
pub enum SnapshotError {
    /// Unparseable, an unknown `schema`, or a binding that is not the
    /// directory it was read for: the partition is quarantined.
    Invalid(String),
    Io(anyhow::Error),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SnapshotError::Invalid(r) => write!(f, "invalid control snapshot: {r}"),
            SnapshotError::Io(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

impl ControlSnapshot {
    /// A fresh, empty snapshot bound to `root`.
    pub fn new(
        partition_key: impl Into<String>,
        scope: SnapshotScope,
        owner_key: impl Into<String>,
        owner_id: impl Into<String>,
        root: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        let now = to_second(now);
        Self {
            schema: SNAPSHOT_SCHEMA,
            partition_key: partition_key.into(),
            scope,
            owner_key: owner_key.into(),
            owner_id: owner_id.into(),
            root: root.into(),
            created_at: now,
            updated_at: now,
            moved_from: None,
            repaired_at: None,
            pending: None,
            notes: BTreeMap::new(),
            tombstones: Vec::new(),
        }
    }

    /// Reads a snapshot file. `Ok(None)` when absent.
    pub fn load(path: &Path) -> Result<Option<Self>, SnapshotError> {
        let bytes = match read_if_exists(path) {
            Ok(Some(b)) => b,
            Ok(None) => return Ok(None),
            Err(e) => return Err(SnapshotError::Io(e)),
        };
        let snapshot: ControlSnapshot = serde_json::from_slice(&bytes)
            .map_err(|e| SnapshotError::Invalid(format!("{}: {e}", path.display())))?;
        if snapshot.schema != SNAPSHOT_SCHEMA {
            return Err(SnapshotError::Invalid(format!(
                "{}: unknown schema {}",
                path.display(),
                snapshot.schema
            )));
        }
        Ok(Some(snapshot))
    }

    /// Whether this snapshot is the one for the partition at `root` with
    /// `partition_key` and `owner_key` (design §7a: a `root` that is not the
    /// directory it was read for is invalid).
    pub fn check_binding(
        &self,
        partition_key: &str,
        owner_key: &str,
        root: &str,
    ) -> Result<(), String> {
        if self.root != root {
            return Err(format!(
                "bound to `{}`, read for `{root}`",
                self.root
            ));
        }
        if self.partition_key != partition_key {
            return Err(format!(
                "partition_key is `{}`, expected `{partition_key}`",
                self.partition_key
            ));
        }
        if self.owner_key != owner_key {
            return Err(format!(
                "owner_key is `{}`, expected `{owner_key}`",
                self.owner_key
            ));
        }
        Ok(())
    }

    /// The JSON bytes as written: pretty, keys in the design's order.
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string());
        text.push('\n');
        text
    }

    /// Replaces the file atomically (0600, in a 0700 directory), stamping
    /// `updated_at`. Compacts first so the file stays bounded.
    pub fn save(&mut self, path: &Path, now: DateTime<Utc>) -> Result<()> {
        self.updated_at = to_second(now);
        self.compact();
        if let Some(dir) = path.parent() {
            create_private_dir(dir)?;
        }
        write_atomic_private(path, self.to_json().as_bytes())
            .with_context(|| format!("Failed to write control snapshot {}", path.display()))
    }

    /// Keeps the snapshot bounded (design §6): duplicate source-suppression
    /// entries are collapsed and duplicate tombstones (one id twice) merged. A
    /// live tombstone is never discarded.
    pub fn compact(&mut self) {
        let mut merged: Vec<Tombstone> = Vec::with_capacity(self.tombstones.len());
        for t in self.tombstones.drain(..) {
            if let Some(existing) = merged.iter_mut().find(|m| m.id == t.id) {
                for s in t.supersedes {
                    if !existing.supersedes.contains(&s) {
                        existing.supersedes.push(s);
                    }
                }
                for s in t.suppressed_sources {
                    if !existing.suppressed_sources.contains(&s) {
                        existing.suppressed_sources.push(s);
                    }
                }
                if existing.origin.rank() < t.origin.rank() {
                    existing.origin = t.origin;
                }
                if t.forgotten_at > existing.forgotten_at {
                    existing.forgotten_at = t.forgotten_at;
                }
            } else {
                let mut t = t;
                let mut seen = Vec::new();
                t.suppressed_sources.retain(|s| {
                    if seen.contains(s) {
                        false
                    } else {
                        seen.push(s.clone());
                        true
                    }
                });
                merged.push(t);
            }
        }
        self.tombstones = merged;
    }

    /// The tombstone for `id`, if the note was forgotten.
    pub fn tombstone(&self, id: &NoteId) -> Option<&Tombstone> {
        self.tombstones.iter().find(|t| &t.id == id)
    }

    /// Every suppressed source id across all tombstones.
    pub fn is_source_suppressed(&self, key: &str) -> bool {
        self.tombstones
            .iter()
            .any(|t| t.suppressed_sources.iter().any(|s| s == key))
    }
}

/// RFC 3339 UTC to the second with `Z`, both ways.
pub(super) mod ts {
    use super::*;

    pub fn serialize<S: serde::Serializer>(
        value: &DateTime<Utc>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format_ts(*value))
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<DateTime<Utc>, D::Error> {
        let s = String::deserialize(deserializer)?;
        parse_ts(&s).map_err(serde::de::Error::custom)
    }
}

/// [`ts`] for a nullable field.
pub(super) mod ts_opt {
    use super::*;

    pub fn serialize<S: serde::Serializer>(
        value: &Option<DateTime<Utc>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(v) => serializer.serialize_some(&format_ts(*v)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<DateTime<Utc>>, D::Error> {
        let s: Option<String> = Option::deserialize(deserializer)?;
        match s {
            Some(s) => parse_ts(&s).map(Some).map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}
