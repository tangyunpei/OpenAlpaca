//! The note file format (design §4): YAML frontmatter between `---` lines and
//! a Markdown body, parsed by a small parser written for exactly the flat
//! shape the design defines, and written by one emitter whose bytes are the
//! canon the revision hashes.
//!
//! **Why not a YAML crate.** The canon must be exact and stable: keys in one
//! order, block lists, RFC 3339 `Z` to the second, `null` for an absent
//! optional, comments dropped, unknown keys preserved verbatim in the order
//! they were read. A general parser would re-type `4812` as an integer and
//! `2026-09-23T10:00:00Z` as whatever it likes; the shape here is small
//! enough — scalars, block lists of scalars, and `sources` as a block list of
//! three-field maps — that owning the parser keeps the round trip byte-exact
//! and adds no dependency edge to the crate.
//!
//! A parse followed by a write of a canonical note is byte-identical; a file
//! written by hand comes out canonical (and its revision changes once). Unknown
//! keys are preserved as the exact lines they occupied.

use super::{
    EMPTY_TITLE_SLUG, MAX_LIST_ENTRIES, MAX_NOTE_BYTES, MAX_SOURCES, MAX_TITLE_CHARS, NOTE_SCHEMA,
    SLUG_MAX_BYTES,
};
use crate::hash::sha256_hex;
use crate::store::slugify;
use chrono::{DateTime, Utc};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

// ============================================================================
// Identity and enums
// ============================================================================

/// A note id: a UUID, written and read in its canonical hyphenated lowercase
/// form only (design §7a). The UUID is the identity; the slug beside it in a
/// file name is presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NoteId(Uuid);

impl NoteId {
    /// A fresh v4 id.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Strict: only the canonical hyphenated lowercase spelling is an id.
    pub fn parse(s: &str) -> Result<Self, String> {
        let uuid = Uuid::parse_str(s).map_err(|e| format!("`{s}` is not a UUID: {e}"))?;
        let canonical = uuid.hyphenated().to_string();
        if canonical != s {
            return Err(format!(
                "`{s}` is not the canonical hyphenated lowercase form of a UUID"
            ));
        }
        Ok(Self(uuid))
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for NoteId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

impl FromStr for NoteId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl serde::Serialize for NoteId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for NoteId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        NoteId::parse(&s).map_err(serde::de::Error::custom)
    }
}

macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $($text => Some($name::$variant),)+
                    _ => None,
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s).ok_or_else(|| {
                    let names: Vec<&str> = Self::ALL.iter().map(|v| v.as_str()).collect();
                    format!("`{s}` is not one of {}", names.join(", "))
                })
            }
        }
    };
}

string_enum! {
    /// `kind` (design §4): the six note kinds.
    NoteKind {
        Fact => "fact",
        Preference => "preference",
        Decision => "decision",
        Procedure => "procedure",
        Reference => "reference",
        Episode => "episode",
    }
}

string_enum! {
    /// Where a note lives (design §4).
    NoteScope {
        Global => "global",
        Project => "project",
    }
}

string_enum! {
    /// `status` (design §4). *Superseded* is derived, never stored.
    NoteStatus {
        Active => "active",
        Candidate => "candidate",
        Archived => "archived",
    }
}

string_enum! {
    /// `sources[].type` (design §4): DB handles, never session-log offsets.
    SourceType {
        Message => "message",
        Task => "task",
    }
}

impl NoteKind {
    /// Design §3 rule 2: a preference stated anywhere is still the user's, so
    /// it defaults to the global partition; every other kind defaults to the
    /// project when the request has one.
    pub fn default_scope(self) -> NoteScope {
        match self {
            NoteKind::Preference => NoteScope::Global,
            _ => NoteScope::Project,
        }
    }
}

/// One `sources` entry: `{type, id, observed_at?}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSource {
    pub kind: SourceType,
    pub id: String,
    pub observed_at: Option<DateTime<Utc>>,
}

impl NoteSource {
    /// The form a tombstone suppresses it by: `message:<rowid>` or
    /// `task:<id>` (design §7a).
    pub fn suppression_key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.id)
    }
}

/// A frontmatter key this build does not know, kept as the exact lines it
/// occupied (newline-terminated) so a supported patch re-emits it unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownField {
    pub key: String,
    pub raw: String,
}

// ============================================================================
// The note
// ============================================================================

/// A parsed note: every frontmatter field of design §4 plus the body, exactly
/// as read. Authority is not here — the control snapshot carries origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: NoteId,
    pub title: String,
    pub kind: NoteKind,
    pub status: NoteStatus,
    pub fact_key: Option<String>,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub observed_at: Option<DateTime<Utc>>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub review_after: Option<DateTime<Utc>>,
    pub pinned: bool,
    pub sources: Vec<NoteSource>,
    pub supersedes: Vec<NoteId>,
    /// Unknown frontmatter keys, in the order they were read.
    pub unknown: Vec<UnknownField>,
    /// The Markdown below the frontmatter, byte-for-byte.
    pub body: String,
}

/// Why a file is not a note. Malformed files stay on disk, excluded, never
/// rewritten (design §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteParseError {
    pub reason: String,
}

impl fmt::Display for NoteParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for NoteParseError {}

fn err<T>(reason: impl Into<String>) -> Result<T, NoteParseError> {
    Err(NoteParseError {
        reason: reason.into(),
    })
}

/// The `fact_key` grammar: `[a-z0-9][a-z0-9._-]{0,63}` (design §4).
pub fn is_valid_fact_key(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    let first_ok = bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit();
    first_ok
        && bytes[1..].iter().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// Lowercase hex SHA-256 of a note file's exact bytes (design §4).
pub fn revision_of(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

/// The knowledge a note body states (design §4's sections): the text of its
/// `## Knowledge` section, up to the next heading — or, for a body without
/// one, the body below its `#` title line. A note learned before Learning
/// v2 L2 is `# {title}\n\n{content}` (design 09 §2.5, §2.6), so the second
/// branch reads its content. Trimmed.
pub fn knowledge_text(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let heading = |line: &str| line.trim_start().starts_with('#');
    if let Some(start) = lines.iter().position(|l| {
        let t = l.trim();
        t.strip_prefix("##").is_some_and(|rest| {
            !rest.starts_with('#') && rest.trim().eq_ignore_ascii_case("knowledge")
        })
    }) {
        let rest = &lines[start + 1..];
        let end = rest.iter().position(|l| heading(l)).unwrap_or(rest.len());
        return rest[..end].join("\n").trim().to_string();
    }
    let mut rest: &[&str] = &lines;
    if let Some(first) = rest.iter().position(|l| !l.trim().is_empty())
        && rest[first].trim_start().starts_with("# ")
    {
        rest = &rest[first + 1..];
    }
    rest.join("\n").trim().to_string()
}

/// Two statements of knowledge compare equal after this (design 09 §2.5):
/// NFKC, case-folded, whitespace runs collapsed to one space, and trailing
/// punctuation removed — "The user edits with vim." and "the user edits
/// with  vim" are one statement; "vim" and "Helix" are not.
pub fn normalize_statement(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let folded: String = text.nfkc().collect::<String>().to_lowercase();
    let collapsed = folded.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .trim_end_matches(|c: char| {
            c.is_ascii_punctuation()
                || c.is_whitespace()
                || matches!(c, '。' | '、' | '」' | '』' | '“' | '”' | '‘' | '’' | '…')
        })
        .to_string()
}

/// Truncates a timestamp to the second, which is all the canon writes.
pub(super) fn to_second(ts: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp(ts.timestamp(), 0).unwrap_or(ts)
}

/// RFC 3339 UTC to the second with `Z` — the one timestamp form the canon
/// writes (design §4).
pub(super) fn format_ts(ts: DateTime<Utc>) -> String {
    ts.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Parses an RFC 3339 timestamp (any offset), normalized to UTC.
pub(super) fn parse_ts(s: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| format!("`{s}` is not an RFC 3339 timestamp: {e}"))
}

impl Note {
    /// The file name the canon gives this note: `<uuid>--<slug>.md`, the slug
    /// being [`slugify`]`(title, 48)` with `note` for a title that leaves
    /// nothing (design §2).
    pub fn file_name(&self) -> String {
        Self::file_name_for(self.id, &self.title)
    }

    pub fn file_name_for(id: NoteId, title: &str) -> String {
        let mut slug = slugify(title, SLUG_MAX_BYTES);
        if slug == "artifact" {
            slug = EMPTY_TITLE_SLUG.to_string();
        }
        format!("{id}--{slug}.md")
    }

    /// The id a note file name starts with, if it has the canon's shape.
    pub fn id_from_file_name(name: &str) -> Option<NoteId> {
        let stem = name.strip_suffix(".md")?;
        let (id, _slug) = stem.split_once("--")?;
        NoteId::parse(id).ok()
    }

    /// The revision of this note's canonical bytes.
    pub fn revision(&self) -> String {
        revision_of(self.serialize().as_bytes())
    }

    /// Write-time validation (design §4 limits, §7a shapes). Parsing does not
    /// apply these: an external file that exceeds them is still read, and
    /// only a service write is refused.
    pub fn validate_for_write(&self) -> Result<(), String> {
        if self.title.trim().is_empty() {
            return Err("title must not be empty".into());
        }
        if self.title.chars().count() > MAX_TITLE_CHARS {
            return Err(format!("title exceeds {MAX_TITLE_CHARS} characters"));
        }
        if self.aliases.len() > MAX_LIST_ENTRIES {
            return Err(format!("aliases exceed {MAX_LIST_ENTRIES} entries"));
        }
        if self.tags.len() > MAX_LIST_ENTRIES {
            return Err(format!("tags exceed {MAX_LIST_ENTRIES} entries"));
        }
        if self.sources.len() > MAX_SOURCES {
            return Err(format!("sources exceed {MAX_SOURCES} entries"));
        }
        if let Some(key) = &self.fact_key
            && !is_valid_fact_key(key)
        {
            return Err(format!(
                "fact_key `{key}` does not match [a-z0-9][a-z0-9._-]{{0,63}}"
            ));
        }
        for s in self.aliases.iter().chain(self.tags.iter()) {
            if s.trim().is_empty() {
                return Err("aliases and tags must not be empty strings".into());
            }
        }
        if self.body.len() > MAX_NOTE_BYTES {
            return Err(format!(
                "body is {} bytes; a note is at most {} bytes (32 KiB)",
                self.body.len(),
                MAX_NOTE_BYTES
            ));
        }
        let total = self.serialize().len();
        if total > MAX_NOTE_BYTES {
            return Err(format!(
                "the note would be {total} bytes with its frontmatter; a note is at most {MAX_NOTE_BYTES} bytes (32 KiB)"
            ));
        }
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Canonical serialization (design §4)
    // ------------------------------------------------------------------------

    /// The canonical bytes: frontmatter keys in the design's order, unknown
    /// keys after them as read, block lists, RFC 3339 `Z`, `null` for an
    /// absent optional, then the body byte-for-byte.
    pub fn serialize(&self) -> String {
        let mut out = String::with_capacity(512 + self.body.len());
        out.push_str("---\n");
        out.push_str(&format!("schema: {NOTE_SCHEMA}\n"));
        out.push_str(&format!("id: {}\n", self.id));
        out.push_str(&format!("title: {}\n", emit_scalar(&self.title)));
        out.push_str(&format!("kind: {}\n", self.kind));
        out.push_str(&format!("status: {}\n", self.status));
        out.push_str(&format!(
            "fact_key: {}\n",
            self.fact_key
                .as_deref()
                .map_or("null".to_string(), emit_scalar)
        ));
        emit_list(&mut out, "aliases", &self.aliases);
        emit_list(&mut out, "tags", &self.tags);
        out.push_str(&format!("created_at: {}\n", format_ts(self.created_at)));
        out.push_str(&format!("updated_at: {}\n", format_ts(self.updated_at)));
        out.push_str(&format!("observed_at: {}\n", emit_opt_ts(self.observed_at)));
        out.push_str(&format!("valid_from: {}\n", emit_opt_ts(self.valid_from)));
        out.push_str(&format!("valid_to: {}\n", emit_opt_ts(self.valid_to)));
        out.push_str(&format!(
            "review_after: {}\n",
            emit_opt_ts(self.review_after)
        ));
        out.push_str(&format!("pinned: {}\n", self.pinned));
        if self.sources.is_empty() {
            out.push_str("sources: []\n");
        } else {
            out.push_str("sources:\n");
            for s in &self.sources {
                out.push_str(&format!("  - type: {}\n", s.kind));
                out.push_str(&format!("    id: {}\n", emit_scalar(&s.id)));
                if let Some(t) = s.observed_at {
                    out.push_str(&format!("    observed_at: {}\n", format_ts(t)));
                }
            }
        }
        if self.supersedes.is_empty() {
            out.push_str("supersedes: []\n");
        } else {
            out.push_str("supersedes:\n");
            for id in &self.supersedes {
                out.push_str(&format!("  - {id}\n"));
            }
        }
        for u in &self.unknown {
            out.push_str(&u.raw);
            if !u.raw.ends_with('\n') {
                out.push('\n');
            }
        }
        out.push_str("---\n");
        out.push_str(&self.body);
        out
    }

    // ------------------------------------------------------------------------
    // Parsing
    // ------------------------------------------------------------------------

    /// Parses a note file. Every refusal names why; a refused file is
    /// *malformed* and stays on disk, excluded (design §4).
    pub fn parse(bytes: &[u8]) -> Result<Note, NoteParseError> {
        if bytes.len() > MAX_NOTE_BYTES {
            return err(format!(
                "oversized: {} bytes, the limit is {MAX_NOTE_BYTES} (32 KiB)",
                bytes.len()
            ));
        }
        let text = match std::str::from_utf8(bytes) {
            Ok(t) => t,
            Err(e) => return err(format!("not UTF-8: {e}")),
        };
        let (frontmatter, body) = split_frontmatter(text)?;
        let entries = parse_entries(frontmatter)?;

        let mut fields = Fields::default();
        let mut unknown = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        for entry in &entries {
            if seen.contains(&entry.key.as_str()) {
                return err(format!("duplicate frontmatter key `{}`", entry.key));
            }
            seen.push(entry.key.as_str());
            match entry.key.as_str() {
                "schema" => fields.schema = Some(entry.scalar()?),
                "id" => fields.id = Some(entry.scalar()?),
                "title" => fields.title = Some(entry.scalar()?),
                "kind" => fields.kind = Some(entry.scalar()?),
                "status" => fields.status = Some(entry.scalar()?),
                "fact_key" => fields.fact_key = Some(entry.scalar()?),
                "aliases" => fields.aliases = Some(entry.list()?),
                "tags" => fields.tags = Some(entry.list()?),
                "created_at" => fields.created_at = Some(entry.scalar()?),
                "updated_at" => fields.updated_at = Some(entry.scalar()?),
                "observed_at" => fields.observed_at = Some(entry.scalar()?),
                "valid_from" => fields.valid_from = Some(entry.scalar()?),
                "valid_to" => fields.valid_to = Some(entry.scalar()?),
                "review_after" => fields.review_after = Some(entry.scalar()?),
                "pinned" => fields.pinned = Some(entry.scalar()?),
                "sources" => fields.sources = Some(entry.sources()?),
                "supersedes" => fields.supersedes = Some(entry.list()?),
                _ => unknown.push(UnknownField {
                    key: entry.key.clone(),
                    raw: entry.raw.clone(),
                }),
            }
        }

        let schema = match fields.schema {
            Some(Scalar::Plain(s)) => s.parse::<u32>().map_err(|_| NoteParseError {
                reason: format!("schema `{s}` is not an integer"),
            })?,
            Some(_) => return err("schema must be a plain integer"),
            None => return err("missing required key `schema`"),
        };
        if schema != NOTE_SCHEMA {
            return err(format!(
                "unknown schema {schema}; this build reads {NOTE_SCHEMA}"
            ));
        }
        let id = match fields.id.and_then(Scalar::text) {
            Some(s) => NoteId::parse(&s).map_err(|e| NoteParseError {
                reason: format!("id: {e}"),
            })?,
            None => return err("missing required key `id`"),
        };
        let title = match fields.title.and_then(Scalar::text) {
            Some(t) if !t.is_empty() => t,
            Some(_) => return err("title must not be empty"),
            None => return err("missing required key `title`"),
        };
        let kind = match fields.kind.and_then(Scalar::text) {
            Some(k) => NoteKind::from_str(&k).map_err(|e| NoteParseError {
                reason: format!("kind: {e}"),
            })?,
            None => return err("missing required key `kind`"),
        };
        let status = match fields.status.and_then(Scalar::text) {
            Some(s) => NoteStatus::from_str(&s).map_err(|e| NoteParseError {
                reason: format!("status: {e}"),
            })?,
            None => return err("missing required key `status`"),
        };
        let created_at = required_ts("created_at", fields.created_at)?;
        let updated_at = required_ts("updated_at", fields.updated_at)?;
        let observed_at = optional_ts("observed_at", fields.observed_at)?;
        let valid_from = optional_ts("valid_from", fields.valid_from)?;
        let valid_to = optional_ts("valid_to", fields.valid_to)?;
        let review_after = optional_ts("review_after", fields.review_after)?;
        // An invalid fact_key in a file is treated as absent (design §4); a
        // service write is refused by `validate_for_write` instead.
        let fact_key = fields
            .fact_key
            .and_then(Scalar::text)
            .filter(|k| is_valid_fact_key(k));
        let pinned = match fields.pinned {
            None => false,
            Some(Scalar::Null) => false,
            Some(Scalar::Plain(s)) => match s.as_str() {
                "true" | "True" | "TRUE" => true,
                "false" | "False" | "FALSE" => false,
                other => return err(format!("pinned `{other}` is not a boolean")),
            },
            Some(Scalar::Quoted(_)) => return err("pinned must be a plain boolean"),
        };
        let mut supersedes = Vec::new();
        for s in fields.supersedes.unwrap_or_default() {
            supersedes.push(NoteId::parse(&s).map_err(|e| NoteParseError {
                reason: format!("supersedes: {e}"),
            })?);
        }

        Ok(Note {
            id,
            title,
            kind,
            status,
            fact_key,
            aliases: fields.aliases.unwrap_or_default(),
            tags: fields.tags.unwrap_or_default(),
            created_at,
            updated_at,
            observed_at,
            valid_from,
            valid_to,
            review_after,
            pinned,
            sources: fields.sources.unwrap_or_default(),
            supersedes,
            unknown,
            body: body.to_string(),
        })
    }
}

fn required_ts(key: &str, value: Option<Scalar>) -> Result<DateTime<Utc>, NoteParseError> {
    match value.and_then(Scalar::text) {
        Some(s) => parse_ts(&s).map_err(|e| NoteParseError {
            reason: format!("{key}: {e}"),
        }),
        None => err(format!("missing required key `{key}`")),
    }
}

fn optional_ts(key: &str, value: Option<Scalar>) -> Result<Option<DateTime<Utc>>, NoteParseError> {
    match value.and_then(Scalar::text) {
        Some(s) => parse_ts(&s).map(Some).map_err(|e| NoteParseError {
            reason: format!("{key}: {e}"),
        }),
        None => Ok(None),
    }
}

#[derive(Default)]
struct Fields {
    schema: Option<Scalar>,
    id: Option<Scalar>,
    title: Option<Scalar>,
    kind: Option<Scalar>,
    status: Option<Scalar>,
    fact_key: Option<Scalar>,
    aliases: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    created_at: Option<Scalar>,
    updated_at: Option<Scalar>,
    observed_at: Option<Scalar>,
    valid_from: Option<Scalar>,
    valid_to: Option<Scalar>,
    review_after: Option<Scalar>,
    pinned: Option<Scalar>,
    sources: Option<Vec<NoteSource>>,
    supersedes: Option<Vec<String>>,
}

// ============================================================================
// The frontmatter grammar
// ============================================================================

/// Splits `---\n<frontmatter>---\n<body>`. The body is everything after the
/// closing line's newline, untouched.
fn split_frontmatter(text: &str) -> Result<(&str, &str), NoteParseError> {
    let after_open = if let Some(rest) = text.strip_prefix("---\n") {
        rest
    } else if let Some(rest) = text.strip_prefix("---\r\n") {
        rest
    } else {
        return err("the file does not start with a `---` frontmatter line");
    };
    let mut offset = 0usize;
    for line in after_open.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
        if trimmed == "---" {
            let frontmatter = &after_open[..offset];
            let body = &after_open[offset + line.len()..];
            return Ok((frontmatter, body));
        }
        offset += line.len();
    }
    err("the frontmatter has no closing `---` line")
}

/// A scalar as written: plain (unquoted), quoted (already unescaped), or null.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scalar {
    Plain(String),
    Quoted(String),
    Null,
}

impl Scalar {
    fn text(self) -> Option<String> {
        match self {
            Scalar::Plain(s) | Scalar::Quoted(s) => Some(s),
            Scalar::Null => None,
        }
    }
}

/// One top-level `key: …` entry with its continuation lines.
#[derive(Debug)]
struct Entry {
    key: String,
    /// The text after `key:` on the key line, trimmed; `None` when empty.
    inline: Option<String>,
    /// Continuation lines (indented or `- ` lines), comments and blank lines
    /// removed, with their original indentation.
    block: Vec<String>,
    /// The exact kept lines of this entry, newline-terminated.
    raw: String,
}

fn is_comment_or_blank(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#')
}

fn parse_entries(frontmatter: &str) -> Result<Vec<Entry>, NoteParseError> {
    let lines: Vec<&str> = frontmatter
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    // `split` on a newline-terminated text yields a trailing empty piece.
    let lines: Vec<&str> = if lines.last() == Some(&"") {
        lines[..lines.len() - 1].to_vec()
    } else {
        lines
    };
    let mut entries = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if is_comment_or_blank(line) {
            i += 1;
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') || line.starts_with('-') {
            return err(format!(
                "frontmatter line {} is not a `key: value` entry: `{line}`",
                i + 1
            ));
        }
        let Some(colon) = line.find(':') else {
            return err(format!("frontmatter line {} has no `:`: `{line}`", i + 1));
        };
        let key = &line[..colon];
        let rest = &line[colon + 1..];
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            return err(format!(
                "frontmatter line {} has an invalid key `{key}`",
                i + 1
            ));
        }
        if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
            return err(format!(
                "frontmatter line {} needs a space after `{key}:`",
                i + 1
            ));
        }
        let inline_text = strip_plain_comment(rest.trim());
        let inline = if inline_text.is_empty() {
            None
        } else {
            Some(inline_text.to_string())
        };
        let mut raw_lines: Vec<&str> = vec![line];
        let mut block: Vec<String> = Vec::new();
        let mut j = i + 1;
        while j < lines.len() {
            let next = lines[j];
            let continuation =
                next.starts_with(' ') || next.starts_with('\t') || next.starts_with('-');
            if is_comment_or_blank(next) {
                j += 1;
                continue;
            }
            if !continuation {
                break;
            }
            raw_lines.push(next);
            block.push(next.to_string());
            j += 1;
        }
        let mut raw = raw_lines.join("\n");
        raw.push('\n');
        entries.push(Entry {
            key: key.to_string(),
            inline,
            block,
            raw,
        });
        i = j;
    }
    Ok(entries)
}

/// YAML cuts a plain scalar at ` #`; quoted scalars are left alone.
fn strip_plain_comment(s: &str) -> &str {
    if s.starts_with('"') || s.starts_with('\'') {
        return s;
    }
    let mut cut = s.len();
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'#' && i > 0 && (bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
            cut = i;
            break;
        }
    }
    s[..cut].trim_end()
}

impl Entry {
    fn scalar(&self) -> Result<Scalar, NoteParseError> {
        if !self.block.is_empty() {
            return err(format!("`{}` must be a scalar, not a block", self.key));
        }
        match &self.inline {
            None => Ok(Scalar::Null),
            Some(text) => parse_scalar(text).map_err(|reason| NoteParseError {
                reason: format!("{}: {reason}", self.key),
            }),
        }
    }

    fn list(&self) -> Result<Vec<String>, NoteParseError> {
        match (&self.inline, self.block.is_empty()) {
            (Some(text), true) => {
                if text == "null" || text == "~" {
                    return Ok(Vec::new());
                }
                parse_flow_list(text).map_err(|reason| NoteParseError {
                    reason: format!("{}: {reason}", self.key),
                })
            }
            (Some(_), false) => err(format!(
                "`{}` has both an inline value and a block",
                self.key
            )),
            (None, true) => Ok(Vec::new()),
            (None, false) => {
                let mut items = Vec::new();
                for line in &self.block {
                    let item = line.trim_start();
                    let Some(rest) = item.strip_prefix('-') else {
                        return err(format!(
                            "`{}` list line is not a `- item`: `{line}`",
                            self.key
                        ));
                    };
                    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
                        return err(format!(
                            "`{}` list line is not a `- item`: `{line}`",
                            self.key
                        ));
                    }
                    let text = strip_plain_comment(rest.trim());
                    match parse_scalar(text).map_err(|reason| NoteParseError {
                        reason: format!("{}: {reason}", self.key),
                    })? {
                        Scalar::Null => {
                            return err(format!("`{}` list has a null item", self.key));
                        }
                        Scalar::Plain(s) | Scalar::Quoted(s) => items.push(s),
                    }
                }
                Ok(items)
            }
        }
    }

    fn sources(&self) -> Result<Vec<NoteSource>, NoteParseError> {
        match (&self.inline, self.block.is_empty()) {
            (Some(text), true) => {
                if text == "[]" || text == "null" || text == "~" {
                    Ok(Vec::new())
                } else {
                    err("sources must be a block list of `{type, id, observed_at?}` maps")
                }
            }
            (Some(_), false) => err("`sources` has both an inline value and a block"),
            (None, true) => Ok(Vec::new()),
            (None, false) => {
                // Group the block into items: a line whose trimmed form starts
                // with `- ` opens one; every following line that does not is a
                // field of it.
                let mut items: Vec<Vec<String>> = Vec::new();
                for line in &self.block {
                    let t = line.trim_start();
                    if let Some(rest) = t.strip_prefix("- ") {
                        items.push(vec![rest.trim().to_string()]);
                    } else if t == "-" {
                        items.push(Vec::new());
                    } else if let Some(item) = items.last_mut() {
                        item.push(t.trim().to_string());
                    } else {
                        return err(format!("sources: `{line}` is not inside a `- ` item"));
                    }
                }
                let mut sources = Vec::new();
                for fields in items {
                    let mut kind = None;
                    let mut id = None;
                    let mut observed_at = None;
                    for field in fields {
                        let field = strip_plain_comment(&field);
                        if field.is_empty() {
                            continue;
                        }
                        let Some((k, v)) = field.split_once(':') else {
                            return err(format!("sources: `{field}` is not `key: value`"));
                        };
                        let v = strip_plain_comment(v.trim());
                        let scalar = parse_scalar(v).map_err(|reason| NoteParseError {
                            reason: format!("sources.{k}: {reason}"),
                        })?;
                        match k.trim() {
                            "type" => {
                                let text = scalar.text().unwrap_or_default();
                                kind = Some(SourceType::from_str(&text).map_err(|e| {
                                    NoteParseError {
                                        reason: format!("sources.type: {e}"),
                                    }
                                })?);
                            }
                            "id" => id = scalar.text(),
                            "observed_at" => {
                                observed_at = match scalar.text() {
                                    Some(s) => Some(parse_ts(&s).map_err(|e| NoteParseError {
                                        reason: format!("sources.observed_at: {e}"),
                                    })?),
                                    None => None,
                                }
                            }
                            other => {
                                return err(format!("sources: unknown field `{other}`"));
                            }
                        }
                    }
                    let (Some(kind), Some(id)) = (kind, id) else {
                        return err("sources: every entry needs `type` and `id`");
                    };
                    if id.is_empty() {
                        return err("sources: `id` must not be empty");
                    }
                    sources.push(NoteSource {
                        kind,
                        id,
                        observed_at,
                    });
                }
                Ok(sources)
            }
        }
    }
}

/// `[]`, or `[a, "b", c]` — one level, scalars only.
fn parse_flow_list(text: &str) -> Result<Vec<String>, String> {
    let inner = text
        .strip_prefix('[')
        .and_then(|t| t.strip_suffix(']'))
        .ok_or_else(|| format!("`{text}` is neither a block list nor a flow list"))?;
    let inner = inner.trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    for piece in split_flow_items(inner) {
        match parse_scalar(piece.trim())? {
            Scalar::Null => return Err("a list item is null".into()),
            Scalar::Plain(s) | Scalar::Quoted(s) => items.push(s),
        }
    }
    Ok(items)
}

/// Splits a flow list's inside on commas that are not inside quotes.
fn split_flow_items(inner: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in inner.char_indices() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if q == '"' && c == '\\' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                } else if c == ',' {
                    items.push(&inner[start..i]);
                    start = i + 1;
                }
            }
        }
    }
    items.push(&inner[start..]);
    items
}

/// A scalar as YAML would read it, within the subset the canon emits: plain,
/// double-quoted (with escapes) or single-quoted (`''` is one quote); `null`
/// and `~` are null.
fn parse_scalar(text: &str) -> Result<Scalar, String> {
    if text.is_empty() || text == "null" || text == "~" || text == "Null" || text == "NULL" {
        return Ok(Scalar::Null);
    }
    if let Some(inner) = text.strip_prefix('"') {
        let inner = inner
            .strip_suffix('"')
            .ok_or_else(|| format!("unterminated double-quoted scalar `{text}`"))?;
        return unescape_double(inner).map(Scalar::Quoted);
    }
    if let Some(inner) = text.strip_prefix('\'') {
        let inner = inner
            .strip_suffix('\'')
            .ok_or_else(|| format!("unterminated single-quoted scalar `{text}`"))?;
        return Ok(Scalar::Quoted(inner.replace("''", "'")));
    }
    if text.starts_with('[') || text.starts_with('{') {
        return Err(format!(
            "`{text}` is a flow collection where a scalar was expected"
        ));
    }
    Ok(Scalar::Plain(text.to_string()))
}

fn unescape_double(inner: &str) -> Result<String, String> {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if hex.len() != 4 {
                    return Err("truncated \\u escape".into());
                }
                let code = u32::from_str_radix(&hex, 16).map_err(|_| format!("bad \\u{hex}"))?;
                let ch = char::from_u32(code).ok_or_else(|| format!("bad \\u{hex}"))?;
                out.push(ch);
            }
            Some(other) => return Err(format!("unknown escape `\\{other}`")),
            None => return Err("dangling backslash".into()),
        }
    }
    Ok(out)
}

// ============================================================================
// Emitting
// ============================================================================

/// Whether YAML needs `s` quoted to read it back as this string.
fn needs_quotes(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    if matches!(s, "null" | "~" | "Null" | "NULL") {
        return true;
    }
    let first = s.chars().next().unwrap_or(' ');
    if first.is_whitespace()
        || s.ends_with(char::is_whitespace)
        || matches!(
            first,
            '-' | '?'
                | ':'
                | ','
                | '['
                | ']'
                | '{'
                | '}'
                | '#'
                | '&'
                | '*'
                | '!'
                | '|'
                | '>'
                | '\''
                | '"'
                | '%'
                | '@'
                | '`'
        )
    {
        return true;
    }
    if s.contains(": ") || s.contains(" #") || s.ends_with(':') {
        return true;
    }
    s.chars().any(|c| c.is_control())
}

/// A scalar in the canon: plain when YAML reads it back unchanged, otherwise
/// double-quoted with escapes.
fn emit_scalar(s: &str) -> String {
    if !needs_quotes(s) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn emit_list(out: &mut String, key: &str, items: &[String]) {
    if items.is_empty() {
        out.push_str(&format!("{key}: []\n"));
        return;
    }
    out.push_str(&format!("{key}:\n"));
    for item in items {
        out.push_str(&format!("  - {}\n", emit_scalar(item)));
    }
}

fn emit_opt_ts(ts: Option<DateTime<Utc>>) -> String {
    ts.map_or("null".to_string(), format_ts)
}

// ============================================================================
// Write inputs
// ============================================================================

/// How a create that names a `supersedes` target treats it (design 09
/// §3.2, S2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplacePolicy {
    /// The supersede is applied as asked, under G2's authority check
    /// (design §4): every save, tool call and user-directed edit.
    #[default]
    Direct,
    /// Learning's (design 09 §2.5), and the chat model's `memory_store`
    /// since owner decision LQ11: a target the user saved, edited by hand
    /// or imported ([`super::Origin::is_saved`]) gets a *suggestion* — the
    /// note is written as a candidate, which retires nothing until the user
    /// confirms it; a replacement of any other target (a learned note)
    /// stands. Carried from Learning v2 L2 on; the file layer honours it
    /// from L3 (S2).
    SuggestUnlessLearned,
}

/// What a `memory_store` carries into the file layer (design §7a). The
/// partition, the actor and the clock are supplied beside it.
#[derive(Debug, Clone, Default)]
pub struct NewNote {
    pub title: String,
    pub body: String,
    pub kind: Option<NoteKind>,
    /// `active` for a save, `candidate` for an extraction from assistant or
    /// tool output.
    pub status: Option<NoteStatus>,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub fact_key: Option<String>,
    pub sources: Vec<NoteSource>,
    pub observed_at: Option<DateTime<Utc>>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    /// `{id, expected_revision}` of the note this one replaces (design §6).
    pub supersedes: Option<(NoteId, String)>,
    /// How `supersedes` is applied (design 09 §3.2, S2).
    pub replace_policy: ReplacePolicy,
}

/// A `memory_update edit` patch (design §7a): an absent field is untouched;
/// `Some(None)` clears a nullable one.
#[derive(Debug, Clone, Default)]
pub struct NotePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub fact_key: Option<Option<String>>,
    pub valid_from: Option<Option<DateTime<Utc>>>,
    pub valid_to: Option<Option<DateTime<Utc>>>,
    pub review_after: Option<Option<DateTime<Utc>>>,
}

impl NotePatch {
    /// Applies the patch. Untouched fields — the body included — are left
    /// byte-for-byte; `updated_at` is the caller's to bump.
    pub fn apply(&self, note: &mut Note) {
        if let Some(t) = &self.title {
            note.title = t.clone();
        }
        if let Some(b) = &self.body {
            note.body = b.clone();
        }
        if let Some(a) = &self.aliases {
            note.aliases = a.clone();
        }
        if let Some(t) = &self.tags {
            note.tags = t.clone();
        }
        if let Some(k) = &self.fact_key {
            note.fact_key = k.clone();
        }
        if let Some(v) = self.valid_from {
            note.valid_from = v;
        }
        if let Some(v) = self.valid_to {
            note.valid_to = v;
        }
        if let Some(v) = self.review_after {
            note.review_after = v;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.body.is_none()
            && self.aliases.is_none()
            && self.tags.is_none()
            && self.fact_key.is_none()
            && self.valid_from.is_none()
            && self.valid_to.is_none()
            && self.review_after.is_none()
    }
}
