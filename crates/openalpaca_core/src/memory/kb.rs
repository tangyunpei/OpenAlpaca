//! KB ingestion pipeline: ingest markdown files into memory as KbChunk entries.

use anyhow::Result;
use openalpaca_storage::repository::MemoryRepository;
use openalpaca_storage::{Database, MemoryKind, MemoryScope, MemorySource};
use std::path::Path;

use super::chunker::chunk_markdown;

pub struct KbIngestResult {
    pub chunks_added: usize,
    pub chunks_skipped: usize,
    pub files_processed: usize,
}

/// Ingest markdown files from a directory into memory as KbChunk entries.
pub fn ingest_directory(
    db: &Database,
    owner_id: &str,
    dir: &Path,
    scope_id: &str,
) -> Result<KbIngestResult> {
    let repo = MemoryRepository::new(db);
    let mut result = KbIngestResult {
        chunks_added: 0,
        chunks_skipped: 0,
        files_processed: 0,
    };

    let md_files = collect_markdown_files(dir)?;

    for file_path in md_files {
        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Skipping {}: {e}", file_path.display());
                continue;
            }
        };

        let relative_path = file_path
            .strip_prefix(dir)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .to_string();

        let chunks = chunk_markdown(&content, &relative_path, 500);

        for chunk in chunks {
            let metadata = serde_json::json!({
                "file": chunk.source_file,
                "section": chunk.section,
                "chunk_index": chunk.index,
            });

            // add() uses INSERT OR IGNORE on (owner_id, content_hash) — deduplication is automatic
            let row_id = repo.add(
                owner_id,
                MemoryKind::KbChunk,
                MemoryScope::Workspace,
                scope_id,
                MemorySource::Ingestion,
                &chunk.content,
                Some(&metadata),
                0.6, // default importance for KB chunks
                0.9, // high confidence for ingested content
            )?;

            if row_id > 0 {
                result.chunks_added += 1;
            } else {
                result.chunks_skipped += 1;
            }
        }

        result.files_processed += 1;
    }

    Ok(result)
}

/// Recursively collect all .md files under a directory.
fn collect_markdown_files(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut files = Vec::new();
    if !dir.is_dir() {
        anyhow::bail!("Not a directory: {}", dir.display());
    }

    fn walk(dir: &Path, files: &mut Vec<std::path::PathBuf>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                walk(&path, files)?;
            } else if path.extension().is_some_and(|e| e == "md") {
                files.push(path);
            }
        }
        Ok(())
    }

    walk(dir, &mut files)?;
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_db() -> Database {
        let dir = tempdir().unwrap();
        Database::open(&dir.path().join("test.db")).unwrap()
    }

    #[test]
    fn test_ingest_directory() {
        let db = test_db();
        let docs_dir = tempdir().unwrap();

        // Create test markdown files
        std::fs::write(
            docs_dir.path().join("readme.md"),
            "## Getting Started\nWelcome to the project.\n\n## Installation\nRun cargo build.",
        )
        .unwrap();
        std::fs::write(
            docs_dir.path().join("guide.md"),
            "## Usage Guide\nHere is how to use the tool.",
        )
        .unwrap();

        let result = ingest_directory(&db, "owner-1", docs_dir.path(), "/workspace/test").unwrap();
        assert_eq!(result.files_processed, 2);
        assert!(result.chunks_added > 0);
        assert_eq!(result.chunks_skipped, 0);
    }

    #[test]
    fn test_ingest_dedup() {
        let db = test_db();
        let docs_dir = tempdir().unwrap();

        std::fs::write(
            docs_dir.path().join("doc.md"),
            "## Section\nSome content here.",
        )
        .unwrap();

        let result1 = ingest_directory(&db, "owner-1", docs_dir.path(), "/ws").unwrap();
        assert!(result1.chunks_added > 0);

        // Re-ingest same directory — should be deduped
        let result2 = ingest_directory(&db, "owner-1", docs_dir.path(), "/ws").unwrap();
        assert_eq!(result2.chunks_added, 0);
        assert!(result2.chunks_skipped > 0);
    }
}
