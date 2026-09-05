//! Artifact models — the classification columns migration 036 adds to
//! `file_assets`.
//!
//! `file_assets` is the one artifact record: `origin` selects the placement
//! strategy (`upload` vs. `produced`) and `kind` is how the client renders the
//! content. The `kind` spellings are the wire contract shared with the GUI's
//! `ArtifactKind` union (`apps/openalpaca-gui/src/lib/api/unbacked.ts`).

use serde::{Deserialize, Serialize};

/// Structured-data and source MIME types outside `text/*` that still render as
/// code ([`ArtifactKind::for_mime`]). Everything neither listed here nor
/// `text/*` — archives, office documents, media, `application/octet-stream` —
/// is `binary`.
const CODE_MIME_TYPES: &[&str] = &[
    "application/json",
    "application/ld+json",
    "application/yaml",
    "application/x-yaml",
    "application/toml",
    "application/x-toml",
    "application/xml",
    "application/javascript",
    "application/ecmascript",
    "application/typescript",
    "application/sql",
    "application/x-python-code",
    "application/x-sh",
];

/// How a client renders an artifact's content.
///
/// Stored in `file_assets.kind`; NULL for legacy uploads that predate 036.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Markdown,
    Code,
    Terminal,
    Table,
    Plan,
    Image,
    Html,
    Binary,
}

impl ArtifactKind {
    /// The stored spelling — also the JSON spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Code => "code",
            Self::Terminal => "terminal",
            Self::Table => "table",
            Self::Plan => "plan",
            Self::Image => "image",
            Self::Html => "html",
            Self::Binary => "binary",
        }
    }

    /// The kind a MIME type projects to (R25).
    ///
    /// Two callers, one map: [`crate::UploadStore::put`] stores it on insert —
    /// an upload arrives with a MIME type and no kind of its own — and the
    /// artifact routes fall back to it for a row whose stored `kind` is NULL
    /// (a pre-036 row, or an upload written before R25), so `kind` is never
    /// `null` on the wire while the client types it non-nullable.
    ///
    /// Total by construction: an unrecognised type is `binary`, the kind that
    /// promises the least about how the bytes render. `image/*` is matched by
    /// prefix; the three MIME types a browser executes script from are *not*
    /// special here — sandboxing them is the content response's job, not the
    /// classification's.
    pub fn for_mime(mime: &str) -> Self {
        let essence = mime
            .split(';')
            .next()
            .unwrap_or(mime)
            .trim()
            .to_ascii_lowercase();
        if essence.starts_with("image/") {
            return Self::Image;
        }
        match essence.as_str() {
            "text/html" | "application/xhtml+xml" => Self::Html,
            "text/markdown" | "text/x-markdown" => Self::Markdown,
            "text/csv" | "text/tab-separated-values" => Self::Table,
            other if other.starts_with("text/") => Self::Code,
            other if CODE_MIME_TYPES.contains(&other) => Self::Code,
            _ => Self::Binary,
        }
    }

    /// Parse a stored spelling. Unknown values are `None` rather than a
    /// silent fallback: `kind` is nullable, and a row written by a newer
    /// daemon must not be mislabelled by an older one.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "markdown" => Some(Self::Markdown),
            "code" => Some(Self::Code),
            "terminal" => Some(Self::Terminal),
            "table" => Some(Self::Table),
            "plan" => Some(Self::Plan),
            "image" => Some(Self::Image),
            "html" => Some(Self::Html),
            "binary" => Some(Self::Binary),
            _ => None,
        }
    }
}

/// Where an artifact came from — the column that selects its placement
/// strategy and, with it, its retention.
///
/// `Upload` rows are user uploads: they count against the upload quota and the
/// orphan sweep may collect them. `Produced` rows are agent output living in
/// the user's own project; they are never garbage-collected and never counted
/// against the upload quota (plan §4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactOrigin {
    /// The `file_assets.origin` column default.
    #[default]
    Upload,
    Produced,
}

impl ArtifactOrigin {
    /// The stored spelling — also the JSON spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Produced => "produced",
        }
    }

    /// Parse a stored spelling. `origin` is `NOT NULL DEFAULT 'upload'`, and an
    /// unrecognised value reads as the default rather than failing a list query.
    pub fn parse(s: &str) -> Self {
        match s {
            "produced" => Self::Produced,
            _ => Self::Upload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every spelling the GUI's `ArtifactKind` union declares
    /// (`apps/openalpaca-gui/src/lib/api/unbacked.ts`).
    const GUI_KINDS: [&str; 8] = [
        "markdown", "code", "terminal", "table", "plan", "image", "html", "binary",
    ];

    #[test]
    fn artifact_kind_round_trips_every_gui_spelling() {
        for spelling in GUI_KINDS {
            let kind =
                ArtifactKind::parse(spelling).unwrap_or_else(|| panic!("{spelling} should parse"));
            assert_eq!(kind.as_str(), spelling);

            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{spelling}\""));
            assert_eq!(
                serde_json::from_str::<ArtifactKind>(&json).unwrap(),
                kind,
                "{spelling} should survive a JSON round trip"
            );
        }
    }

    /// R25: the MIME → kind projection the upload writer stores on insert and
    /// the artifact routes fall back to, so `kind` is never `null` on the wire.
    #[test]
    fn artifact_kind_for_mime_follows_the_r25_table() {
        for (mime, expected) in [
            ("image/png", ArtifactKind::Image),
            ("image/jpeg", ArtifactKind::Image),
            // Active content, but still an image as far as rendering goes —
            // the sandboxing of it is the content response's job, not `kind`'s.
            ("image/svg+xml", ArtifactKind::Image),
            ("text/html", ArtifactKind::Html),
            ("application/xhtml+xml", ArtifactKind::Html),
            ("text/markdown", ArtifactKind::Markdown),
            ("text/x-markdown", ArtifactKind::Markdown),
            ("text/csv", ArtifactKind::Table),
            ("text/tab-separated-values", ArtifactKind::Table),
            ("text/plain", ArtifactKind::Code),
            ("text/x-python", ArtifactKind::Code),
            ("text/yaml", ArtifactKind::Code),
            ("application/json", ArtifactKind::Code),
            ("application/yaml", ArtifactKind::Code),
            ("application/toml", ArtifactKind::Code),
            ("application/xml", ArtifactKind::Code),
            ("application/javascript", ArtifactKind::Code),
            ("application/pdf", ArtifactKind::Binary),
            ("application/zip", ArtifactKind::Binary),
            ("application/octet-stream", ArtifactKind::Binary),
            ("audio/mpeg", ArtifactKind::Binary),
            ("", ArtifactKind::Binary),
        ] {
            assert_eq!(ArtifactKind::for_mime(mime), expected, "mime {mime:?}");
        }
    }

    #[test]
    fn artifact_kind_for_mime_ignores_parameters_and_case() {
        assert_eq!(
            ArtifactKind::for_mime("text/HTML; charset=utf-8"),
            ArtifactKind::Html
        );
        assert_eq!(
            ArtifactKind::for_mime("  text/markdown  "),
            ArtifactKind::Markdown
        );
    }

    /// Whatever the mime, the projection is a spelling the client's union
    /// declares — a `kind` derived at read time is as valid as a stored one.
    #[test]
    fn artifact_kind_for_mime_only_ever_yields_a_gui_spelling() {
        for mime in [
            "image/png",
            "text/html",
            "text/markdown",
            "text/csv",
            "text/plain",
            "application/json",
            "application/octet-stream",
            "nonsense",
        ] {
            assert!(GUI_KINDS.contains(&ArtifactKind::for_mime(mime).as_str()));
        }
    }

    #[test]
    fn artifact_kind_rejects_unknown_spellings() {
        assert_eq!(ArtifactKind::parse("Markdown"), None);
        assert_eq!(ArtifactKind::parse("diff"), None);
        assert_eq!(ArtifactKind::parse(""), None);
        assert!(serde_json::from_str::<ArtifactKind>("\"diff\"").is_err());
    }

    #[test]
    fn artifact_origin_round_trips() {
        for origin in [ArtifactOrigin::Upload, ArtifactOrigin::Produced] {
            let spelling = origin.as_str();
            assert_eq!(ArtifactOrigin::parse(spelling), origin);
            assert_eq!(
                serde_json::to_string(&origin).unwrap(),
                format!("\"{spelling}\"")
            );
        }
        assert_eq!(ArtifactOrigin::default(), ArtifactOrigin::Upload);
        // The column default, and anything unexpected, reads as an upload.
        assert_eq!(ArtifactOrigin::parse("nonsense"), ArtifactOrigin::Upload);
    }
}
