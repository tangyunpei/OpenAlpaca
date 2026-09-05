//! Artifact and upload path grammar (plan §4.2, §1.4).
//!
//! Every function here is pure: no I/O beyond the read-only directory checks
//! [`confine_to_root`] needs to canonicalize an existing ancestor. Nothing in
//! this file writes a file, opens the database, or touches a route — that is
//! the writer's job (Phase 2 items 3-6), built on top of this grammar.
//!
//! ```text
//! run_dir  := <YYYY-MM-DD> "-" slug(task_title, 48) "-" taskid[0..8]      ≤ 68 bytes
//! file     := NN "-" slug(name, 60) "." ext                                ≤ 72 bytes
//! slug     := [a-z0-9]+ ("-" [a-z0-9]+)*
//! ext      := [a-z0-9]{1,8}
//! version  := <run_dir>/.versions/<stem>/v<N>.<ext>
//! ```
//!
//! Traversal safety falls out of the grammar itself — a separator cannot
//! survive [`slugify`] — but every placement function still runs its result
//! through [`confine_to_root`] (the canonicalize-the-existing-parent technique
//! also used at `openalpaca_core::tools::builtins::helpers::resolve_workspace_path_for_write`;
//! reimplemented here because `openalpaca_storage` is a leaf crate below
//! `openalpaca_core` in the dependency graph and cannot import it).
//!
//! ## `artifact_extension`'s per-kind acceptable-extension sets (R21)
//!
//! [`artifact_extension`]'s three fallback tiers (name hint → kind default →
//! mime) each select an extension only from the *declared kind's own*
//! acceptable set — a flat, kind-blind allow-list previously let a
//! model-supplied `name_hint` or `mime` force an extension unrelated to (and
//! more dangerous than) the artifact's own `kind`, e.g. a `Table` artifact
//! written to disk as `.html` on the strength of a `name_hint` of
//! `"output.html"` alone. Active-content extensions (capable of running code
//! or a script when opened directly — `html`, `htm`, `xhtml`, `svg`, `sh`,
//! `bash`, `zsh`, `command`, `ps1`, `php`, `exe`, `bat`, `cmd`, `scr`, `jar`,
//! `app`, `dmg`, `pkg`) are reachable only through a kind whose own set names
//! them — today that means `html`/`htm` only under [`ArtifactKind::Html`],
//! and `js`/`jsx`/`ts`/`tsx` only under [`ArtifactKind::Code`] (ordinary
//! source text, not directly executed by a double-click, unlike a `.sh`/
//! `.command` script once its executable bit is set). No kind grants
//! `sh`/`bash`/`zsh`/`ps1`/`php`/`command` or any packaged-executable
//! extension — see [`kind_allowed_extensions`] for the reasoning behind that
//! choice where the ruling left it open.
//!
//! | Kind | Acceptable extensions (name hint / mime tiers) | Kind default |
//! |---|---|---|
//! | `Markdown`, `Plan` | `md` `markdown` `mdx` `txt` `rst` `adoc` `tex` `pdf` `mmd` | `md` |
//! | `Terminal` | `log` `txt` | `log` |
//! | `Table` | `csv` `tsv` `json` | `csv` |
//! | `Code` | `py` `js` `jsx` `ts` `tsx` `rs` `go` `java` `kt` `kts` `c` `h` `cpp` `hpp` `cs` `rb` `swift` `sql` `lua` `r` `scala` `pl` `css` `scss` `less` `vue` `svelte` `json` `yaml` `yml` `toml` `xml` `ini` `conf` `cfg` `env` `diff` `patch` `lock` `ipynb` | none (→ mime → `bin`) |
//! | `Image` | `png` `jpg` `jpeg` `gif` `webp` `bmp` `ico` `tiff` | none (→ mime → `bin`) |
//! | `Html` | `html` `htm` | `html` |
//! | `Binary` | `zip` `tar` `gz` `tgz` `7z` `pdf` `doc` `docx` `xls` `xlsx` `ppt` `pptx` | none (→ mime → `bin`) |
//!
//! There is no `ArtifactKind::Document` variant — the "Document" example in
//! the ruling's test list is realized against `Markdown`/`Plan`, the two
//! existing kinds closest to free-form prose (both already default to `md`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use super::{ContentKind, StoreScope, content_dir};
use crate::models::ArtifactKind;

// ============================================================================
// Grammar constants
// ============================================================================

/// `slug(task_title, 48)` — leaves room for `<date>-` (11 bytes) + `-<taskid8>`
/// (9 bytes) so `run_dir` never exceeds 68 bytes for any well-formed title.
const RUN_DIR_TITLE_SLUG_BYTES: usize = 48;

/// `slug(name, 60)` — leaves room for `NN-` (3 bytes) + `.` + `ext` (≤ 9 bytes)
/// so `file` never exceeds 72 bytes for a two-digit sequence number.
const FILE_NAME_SLUG_BYTES: usize = 60;

/// `taskid[0..8]` — task ids are UUIDv4 (`Uuid::new_v4().to_string()`), so the
/// first 8 characters are the first hyphen-delimited group, e.g. `3f2a1b7c`.
const TASK_ID_PREFIX_CHARS: usize = 8;

/// `slugify`'s empty-input fallback.
const DEFAULT_SLUG: &str = "artifact";

/// `artifact_extension`'s last-resort default, and `sanitize`'s fallback for
/// anything that doesn't fit the `ext := [a-z0-9]{1,8}` grammar.
const DEFAULT_EXTENSION: &str = "bin";

/// Windows reserved device names (case-insensitive on Windows; `slugify`
/// already lowercases before this check runs). `com10`/`lpt10` are not
/// reserved — only the single-digit forms are.
const RESERVED_DEVICE_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Explicit MIME → extension mappings for `artifact_extension`'s third
/// precedence step. Matched against the MIME type with any `;charset=...`
/// parameter stripped, case-insensitively.
const MIME_EXTENSIONS: &[(&str, &str)] = &[
    ("text/markdown", "md"),
    ("text/x-markdown", "md"),
    ("text/plain", "txt"),
    ("text/html", "html"),
    ("text/csv", "csv"),
    ("text/tab-separated-values", "tsv"),
    ("application/json", "json"),
    ("application/yaml", "yaml"),
    ("text/yaml", "yaml"),
    ("application/toml", "toml"),
    ("application/pdf", "pdf"),
    ("application/zip", "zip"),
    ("text/javascript", "js"),
    ("application/javascript", "js"),
    ("text/x-python", "py"),
    ("application/x-python-code", "py"),
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/gif", "gif"),
    ("image/svg+xml", "svg"),
    ("image/webp", "webp"),
    ("image/bmp", "bmp"),
    ("image/tiff", "tiff"),
    ("image/x-icon", "ico"),
];

// ============================================================================
// slugify
// ============================================================================

/// The slugifier: pure and total — every input produces a non-empty, valid
/// `slug := [a-z0-9]+ ("-" [a-z0-9]+)*` of at most `max_bytes` bytes (plus, in
/// the rare case a reserved device name survives truncation, one more byte
/// for its `_` guard — the guard is applied last, per the grammar, and is not
/// itself budgeted).
///
/// Pipeline: NFKD-decompose → drop combining marks → fold to lowercase ASCII,
/// treating anything that isn't `[a-z0-9]` (spaces, punctuation, and any
/// remaining untranslatable non-Latin code point alike) as a separator and
/// collapsing separator runs to a single `-` → trim → truncate on a char
/// boundary → `artifact` if that leaves nothing → `_`-prefix a Windows
/// reserved device name.
pub fn slugify(input: &str, max_bytes: usize) -> String {
    let mut collapsed = String::with_capacity(input.len());
    let mut last_was_sep = true; // swallow leading separators — never emit one
    for c in input.nfkd().filter(|c| !is_combining_mark(*c)) {
        let lower = c.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            collapsed.push(lower);
            last_was_sep = false;
        } else if !last_was_sep {
            collapsed.push('-');
            last_was_sep = true;
        }
    }
    trim_trailing_hyphen(&mut collapsed);

    let mut truncated = truncate_at_char_boundary(&collapsed, max_bytes).to_string();
    trim_trailing_hyphen(&mut truncated);

    if truncated.is_empty() {
        return DEFAULT_SLUG.to_string();
    }

    guard_reserved_name(truncated)
}

fn trim_trailing_hyphen(s: &mut String) {
    while s.ends_with('-') {
        s.pop();
    }
}

fn guard_reserved_name(slug: String) -> String {
    if RESERVED_DEVICE_NAMES.contains(&slug.as_str()) {
        format!("_{slug}")
    } else {
        slug
    }
}

/// Truncates `s` to at most `max_bytes` bytes, backing off to the nearest
/// valid UTF-8 char boundary rather than panicking. `slugify`'s own output is
/// ASCII by the time it reaches this call (every non-ASCII code point was
/// already folded to a separator above), so this never actually has to back
/// off in practice — it is written to be correct for any input regardless,
/// and is exercised directly with multi-byte input in the test suite.
fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ============================================================================
// confine_to_root
// ============================================================================

/// Confines `candidate` to `root`: canonicalizes `root` (which must already
/// exist) and the nearest *existing* ancestor of `candidate`, then rejects the
/// path unless that canonicalized ancestor is `root` or lives under it. The
/// non-existing tail (typically the leaf being about to be created) is
/// re-appended verbatim onto the canonicalized ancestor.
///
/// This is belt-and-braces: the grammar these paths are built from cannot
/// itself produce a `..` or a separator, so escape can only happen if a
/// symlink was planted somewhere under the store root pointing outside it —
/// exactly what canonicalizing the ancestor (rather than trusting the
/// non-canonical join) catches.
pub fn confine_to_root(root: &Path, candidate: &Path) -> Result<PathBuf> {
    if !root.is_absolute() {
        bail!(
            "confine_to_root: root must be absolute, got {}",
            root.display()
        );
    }
    if !candidate.is_absolute() {
        bail!(
            "confine_to_root: candidate must be absolute, got {}",
            candidate.display()
        );
    }
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!(
            "confine_to_root: path contains a '..' component: {}",
            candidate.display()
        );
    }

    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("store root does not exist: {}", root.display()))?;

    let mut existing: &Path = candidate;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .with_context(|| format!("path has no existing ancestor: {}", candidate.display()))?;
        tail.push(name);
        existing = existing
            .parent()
            .with_context(|| format!("path has no existing ancestor: {}", candidate.display()))?;
    }

    let canonical_existing = existing
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", existing.display()))?;

    if canonical_existing != canonical_root && !canonical_existing.starts_with(&canonical_root) {
        bail!(
            "path escapes the store root: {} is not under {}",
            candidate.display(),
            root.display()
        );
    }

    let mut result = canonical_existing;
    for component in tail.into_iter().rev() {
        result.push(component);
    }
    Ok(result)
}

// ============================================================================
// Artifact grammar
// ============================================================================

fn format_date(created: DateTime<Utc>) -> String {
    created.format("%Y-%m-%d").to_string()
}

/// First `TASK_ID_PREFIX_CHARS` characters of `task_id`, or the whole string
/// if it's shorter. Sliced on `char` boundaries so an unexpected non-ASCII
/// task id (there shouldn't be one — task ids are UUIDs) can't panic.
fn id_prefix(task_id: &str) -> &str {
    match task_id.char_indices().nth(TASK_ID_PREFIX_CHARS) {
        Some((idx, _)) => &task_id[..idx],
        None => task_id,
    }
}

/// `<artifacts>/<YYYY-MM-DD>-<slug(task_title,48)>-<taskid[0..8]>` — the
/// directory a task's produced artifacts land in. Does not create the
/// directory; the writer does that on first use. `content_dir` creates
/// `artifacts/` itself (and, for a project scope, seeds the store first), so
/// the confinement canonicalization always has an existing ancestor to work
/// from.
pub fn run_dir(
    scope: &StoreScope,
    created: DateTime<Utc>,
    task_title: &str,
    task_id: &str,
) -> Result<PathBuf> {
    let artifacts_root = content_dir(scope, ContentKind::Artifacts)?;
    let name = format!(
        "{}-{}-{}",
        format_date(created),
        slugify(task_title, RUN_DIR_TITLE_SLUG_BYTES),
        id_prefix(task_id)
    );
    confine_to_root(&artifacts_root, &artifacts_root.join(name))
}

/// `<artifacts>/loose/<YYYY-MM-DD>` — where artifacts produced outside any
/// task (main-loop chat) land (plan §4.1).
pub fn loose_dir(scope: &StoreScope, created: DateTime<Utc>) -> Result<PathBuf> {
    let artifacts_root = content_dir(scope, ContentKind::Artifacts)?;
    let candidate = artifacts_root.join("loose").join(format_date(created));
    confine_to_root(&artifacts_root, &candidate)
}

/// `NN-<slug(title,60)>.<ext>` — `seq` is zero-padded to two digits, widening
/// to three (and beyond) past 99 rather than truncating.
pub fn artifact_file_name(seq: u32, title: &str, ext: &str) -> String {
    let slug = slugify(title, FILE_NAME_SLUG_BYTES);
    let ext = normalize_ext_or_default(ext);
    format!("{seq:02}-{slug}.{ext}")
}

/// The `NN` of `NN-<slug>.<ext>` — the inverse of the sequence prefix
/// [`artifact_file_name`] and [`upload_file_name`] write, and how a writer
/// reads the next free sequence out of a directory's existing rows.
pub fn leading_sequence(file_name: &str) -> Option<u32> {
    let digits: String = file_name.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// `<run_dir>/.versions/<stem>/v<N>.<ext>`, where `stem` and `ext` are the
/// head file's own file stem and extension.
pub fn version_file_path(head_path: &Path, version: u32) -> Result<PathBuf> {
    let parent = head_path
        .parent()
        .with_context(|| format!("head path has no parent directory: {}", head_path.display()))?;
    let stem = head_path
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("head path has no file stem: {}", head_path.display()))?;
    let file_name = match head_path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("v{version}.{ext}"),
        None => format!("v{version}"),
    };
    Ok(parent.join(".versions").join(stem).join(file_name))
}

/// Extension precedence: an allow-listed extension on the model-supplied
/// `name_hint` → a fixed extension for `kind` → a `mime` mapping → `bin`.
/// R21: every tier is kind-constrained — see the module docs' per-kind
/// table. A name hint or mime value naming an extension outside the
/// declared `kind`'s own acceptable set is not honoured at that tier; it
/// falls through exactly as if it had been absent.
pub fn artifact_extension(
    kind: ArtifactKind,
    mime: Option<&str>,
    name_hint: Option<&str>,
) -> String {
    if let Some(ext) = name_hint.and_then(|name| extension_from_allowed_name(kind, name)) {
        return ext;
    }
    if let Some(ext) = kind_default_extension(kind) {
        return ext.to_string();
    }
    if let Some(ext) = mime.and_then(|m| mime_extension_for_kind(kind, m)) {
        return ext;
    }
    DEFAULT_EXTENSION.to_string()
}

fn kind_default_extension(kind: ArtifactKind) -> Option<&'static str> {
    match kind {
        ArtifactKind::Markdown | ArtifactKind::Plan => Some("md"),
        ArtifactKind::Html => Some("html"),
        ArtifactKind::Terminal => Some("log"),
        ArtifactKind::Table => Some("csv"),
        // Language- or format-specific: no single default. Fall through to
        // the mime map, then `bin`.
        ArtifactKind::Code | ArtifactKind::Image | ArtifactKind::Binary => None,
    }
}

/// `artifact_extension`'s per-kind acceptable-extension sets (R21) — see the
/// module docs' table for the rendered version of the same data plus the
/// reasoning behind it. Both the name-hint tier and the mime tier ([see
/// `mime_extension_for_kind`]) select only from the declared kind's own set
/// here; nothing outside it is ever honoured regardless of tier.
///
/// Two deliberate, documented choices where the ruling left the answer open:
///
/// - `sh`/`bash`/`zsh`/`ps1`/`php`/`command` are **not** granted to `Code`,
///   even though a `Code` artifact could legitimately be shell-script source.
///   Unlike `js`/`ts` (inert as plain text; a browser or Node must be told to
///   run them), a `.sh`/`.command` file can be made to execute via a simple
///   double-click once its executable bit is set — the more restrictive
///   reading, chosen because the ruling left this ambiguous and the
///   escalation instruction is to prefer the restrictive set when in doubt.
/// - There is no `ArtifactKind::Document` variant. `Markdown` and `Plan` —
///   the two kinds already defaulting to `md` — share the "Document" set
///   (`md`, `markdown`, `mdx`, `txt`, `rst`, `adoc`, `tex`, `pdf`, `mmd`).
fn kind_allowed_extensions(kind: ArtifactKind) -> &'static [&'static str] {
    match kind {
        ArtifactKind::Markdown | ArtifactKind::Plan => &[
            "md", "markdown", "mdx", "txt", "rst", "adoc", "tex", "pdf", "mmd",
        ],
        ArtifactKind::Terminal => &["log", "txt"],
        ArtifactKind::Table => &["csv", "tsv", "json"],
        ArtifactKind::Code => &[
            "py", "js", "jsx", "ts", "tsx", "rs", "go", "java", "kt", "kts", "c", "h", "cpp",
            "hpp", "cs", "rb", "swift", "sql", "lua", "r", "scala", "pl", "css", "scss", "less",
            "vue", "svelte", "json", "yaml", "yml", "toml", "xml", "ini", "conf", "cfg", "env",
            "diff", "patch", "lock", "ipynb",
        ],
        // Raster formats only — `svg` is active content (can embed a
        // `<script>`) and is deliberately excluded; there is no `Svg` kind
        // for it to be reachable through.
        ArtifactKind::Image => &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tiff"],
        ArtifactKind::Html => &["html", "htm"],
        ArtifactKind::Binary => &[
            "zip", "tar", "gz", "tgz", "7z", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
        ],
    }
}

fn mime_extension(mime: &str) -> Option<String> {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase();
    if let Some((_, ext)) = MIME_EXTENSIONS.iter().find(|(m, _)| *m == mime) {
        return Some(ext.to_string());
    }
    // Heuristic fallback for anything not in the explicit table: the
    // subtype, minus a leading "x-" facet and any "+suffix" (RFC 6839
    // structured syntax suffix, e.g. "svg+xml"), when that alone already
    // fits the `ext` grammar.
    let subtype = mime.split('/').next_back()?;
    let subtype = subtype.strip_prefix("x-").unwrap_or(subtype);
    let subtype = subtype.split('+').next().unwrap_or(subtype);
    normalize_ext(subtype)
}

/// [`mime_extension`], constrained to `kind`'s own acceptable set (R21) —
/// applies to both the explicit-table hit and the heuristic fallback alike,
/// since neither channel is more trustworthy than the other once an upload
/// or a tool result supplies the `mime` value.
fn mime_extension_for_kind(kind: ArtifactKind, mime: &str) -> Option<String> {
    let ext = mime_extension(mime)?;
    kind_allowed_extensions(kind)
        .contains(&ext.as_str())
        .then_some(ext)
}

/// The extension on `name`, if present and within `kind`'s own acceptable
/// set (R21) — `name` is otherwise-untrusted model output, so the declared
/// `kind` (not the full `ext` grammar) decides whether it's honoured;
/// anything else falls through to the `kind`/`mime` tiers.
fn extension_from_allowed_name(kind: ArtifactKind, name: &str) -> Option<String> {
    let raw = Path::new(name).extension()?.to_str()?;
    let normalized = normalize_ext(raw)?;
    kind_allowed_extensions(kind)
        .contains(&normalized.as_str())
        .then_some(normalized)
}

/// Lowercases and validates against `ext := [a-z0-9]{1,8}` (after stripping a
/// leading `.`, so callers may pass either `"md"` or `".md"`). `None` if it
/// doesn't fit.
fn normalize_ext(ext: &str) -> Option<String> {
    let trimmed = ext.trim_start_matches('.');
    if trimmed.is_empty() || trimmed.len() > 8 {
        return None;
    }
    if !trimmed.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

/// [`normalize_ext`], defaulting to `bin` instead of failing.
fn normalize_ext_or_default(ext: &str) -> String {
    normalize_ext(ext).unwrap_or_else(|| DEFAULT_EXTENSION.to_string())
}

// ============================================================================
// D2 upload placement
// ============================================================================

/// `<uploads>/<YYYY-MM-DD>`.
pub fn upload_dir(scope: &StoreScope, created: DateTime<Utc>) -> Result<PathBuf> {
    let uploads_root = content_dir(scope, ContentKind::Uploads)?;
    let candidate = uploads_root.join(format_date(created));
    confine_to_root(&uploads_root, &candidate)
}

/// `NN-<slug(original_name,60)>.<ext>` — `ext` is the original file's own
/// extension (grammar-validated, defaulting to `bin`), not looked up through
/// `artifact_extension`'s allow-list: an upload's own name is not model
/// output to be second-guessed, only sanitized into the grammar.
pub fn upload_file_name(seq: u32, original_name: &str) -> String {
    let path = Path::new(original_name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(original_name);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(normalize_ext_or_default)
        .unwrap_or_else(|| DEFAULT_EXTENSION.to_string());
    let slug = slugify(stem, FILE_NAME_SLUG_BYTES);
    format!("{seq:02}-{slug}.{ext}")
}

#[cfg(test)]
mod tests;
