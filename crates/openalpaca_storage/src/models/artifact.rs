//! Artifact models — the classification columns migration 036 adds to
//! `file_assets`.
//!
//! `file_assets` is the one artifact record: `origin` selects the placement
//! strategy (`upload` vs. `produced`) and `kind` is how the client renders the
//! content. The `kind` spellings are the wire contract shared with the GUI's
//! `ArtifactKind` union (`apps/openalpaca-gui/src/lib/api/unbacked.ts`).

use serde::{Deserialize, Serialize};

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
