//! Context injection for skill invocations.
//!
//! Reads files, glob patterns, and (future) shell commands from a skill's
//! `context.sources` configuration and assembles them into a formatted context
//! block respecting `budget_tokens` (estimated as chars/4).

use crate::middleware::skill::{ContextConfig, ContextSource};
use std::path::{Path, PathBuf};

/// Inject context from a skill's `context.sources`.
///
/// Returns a formatted context block. Each source is prefixed with a
/// `--- context: <path> ---` header. The total output is capped at
/// `budget_tokens * 4` characters (or 16 000 chars if budget is 0).
pub async fn inject_skill_context(
    context_config: &ContextConfig,
    skill_dir: &Path,
) -> Result<String, String> {
    if context_config.sources.is_empty() {
        return Ok(String::new());
    }

    let budget_chars = if context_config.budget_tokens > 0 {
        context_config.budget_tokens * 4
    } else {
        16000 // default ~4000 tokens
    };
    let mut injected = String::new();
    let mut remaining = budget_chars;

    for source in &context_config.sources {
        if remaining == 0 {
            break;
        }
        match source {
            ContextSource::File { path, max_bytes } => {
                // Security: reject path traversal attempts (P1-3)
                if path.contains("..") {
                    tracing::warn!(
                        "Context injection: path traversal blocked for '{}'",
                        path
                    );
                    continue;
                }
                let full_path = skill_dir.join(path);
                // Verify the resolved path is still within the skill directory
                let canonical = match tokio::fs::canonicalize(&full_path).await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(
                            "Context injection: failed to resolve '{}': {}",
                            path,
                            e
                        );
                        continue;
                    }
                };
                let canonical_skill_dir = match tokio::fs::canonicalize(skill_dir).await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(
                            "Context injection: failed to resolve skill dir: {}",
                            e
                        );
                        continue;
                    }
                };
                if !canonical.starts_with(&canonical_skill_dir) {
                    tracing::warn!(
                        "Context injection: path '{}' escapes skill directory (blocked)",
                        path
                    );
                    continue;
                }
                match tokio::fs::read_to_string(&canonical).await {
                    Ok(content) => {
                        let limit = (*max_bytes).min(remaining);
                        let chunk: String = content.chars().take(limit).collect();
                        remaining = remaining.saturating_sub(chunk.len());
                        injected.push_str(&format!("--- context: {} ---\n{}\n", path, chunk));
                    }
                    Err(e) => {
                        tracing::warn!("Context injection: failed to read '{}': {}", path, e);
                    }
                }
            }
            ContextSource::FileGlob {
                pattern,
                max_files,
                max_bytes_each,
            } => match find_matching_files(skill_dir, pattern, *max_files) {
                Ok(files) => {
                    for file_path in files {
                        if remaining == 0 {
                            break;
                        }
                        let limit = (*max_bytes_each).min(remaining);
                        match tokio::fs::read_to_string(&file_path).await {
                            Ok(content) => {
                                let chunk: String = content.chars().take(limit).collect();
                                remaining = remaining.saturating_sub(chunk.len());
                                let rel = file_path
                                    .strip_prefix(skill_dir)
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_else(|_| file_path.display().to_string());
                                injected
                                    .push_str(&format!("--- context: {} ---\n{}\n", rel, chunk));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Context glob: failed to read '{}': {}",
                                    file_path.display(),
                                    e
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Context glob pattern '{}': {}", pattern, e);
                }
            },
            ContextSource::Shell { command, max_bytes } => {
                // Shell context injection is deferred — requires sandbox/permissions check.
                // Will be gated by permissions.sandbox in a future phase.
                tracing::debug!(
                    "Context shell injection deferred (requires sandbox check): {}",
                    command
                );
                let _ = max_bytes;
            }
        }
    }
    Ok(injected)
}

/// Walk a directory tree and collect files matching a simple glob pattern.
fn find_matching_files(
    base: &Path,
    pattern: &str,
    max_files: usize,
) -> Result<Vec<PathBuf>, String> {
    let mut results = Vec::new();
    walk_dir_glob(base, base, pattern, &mut results, max_files);
    Ok(results)
}

fn walk_dir_glob(
    root: &Path,
    current: &Path,
    pattern: &str,
    results: &mut Vec<PathBuf>,
    max: usize,
) {
    if results.len() >= max {
        return;
    }
    let entries = match std::fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        if results.len() >= max {
            break;
        }
        let path = entry.path();
        if path.is_dir() {
            walk_dir_glob(root, &path, pattern, results, max);
        } else if matches_glob_pattern(&path, root, pattern) {
            results.push(path);
        }
    }
}

fn matches_glob_pattern(path: &Path, root: &Path, pattern: &str) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let rel_str = relative.to_string_lossy();
    simple_glob_match(pattern, &rel_str)
}

/// Simple glob matcher supporting `**` (recursive) and `*` (single-segment wildcard).
fn simple_glob_match(pattern: &str, text: &str) -> bool {
    if pattern.contains("**/") {
        let parts: Vec<&str> = pattern.splitn(2, "**/").collect();
        if parts.len() == 2 {
            // ** matches any number of directories
            return text.ends_with(parts[1]) || simple_glob_match(parts[1], text);
        }
    }
    // Simple * wildcard matching
    if pattern.contains('*') {
        let parts: Vec<&str> = pattern.split('*').collect();
        if parts.len() == 2 {
            return text.starts_with(parts[0]) && text.ends_with(parts[1]);
        }
    }
    text == pattern
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
