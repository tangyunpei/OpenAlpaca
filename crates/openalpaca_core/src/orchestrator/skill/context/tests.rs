use super::*;
use crate::middleware::skill::{ContextConfig, ContextSource, SummarizeConfig};
use std::io::Write;
use tempfile::TempDir;

fn default_context_config() -> ContextConfig {
    ContextConfig {
        sources: Vec::new(),
        summarize: SummarizeConfig::default(),
        budget_tokens: 0,
    }
}

#[tokio::test]
async fn test_empty_sources_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let config = default_context_config();
    let result = inject_skill_context(&config, tmp.path()).await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn test_file_context_injection() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("data.txt");
    let mut f = std::fs::File::create(&file_path).unwrap();
    f.write_all(b"Hello context world").unwrap();

    let config = ContextConfig {
        sources: vec![ContextSource::File {
            path: "data.txt".to_string(),
            max_bytes: 50_000,
        }],
        budget_tokens: 0,
        ..default_context_config()
    };

    let result = inject_skill_context(&config, tmp.path()).await.unwrap();
    assert!(result.contains("--- context: data.txt ---"));
    assert!(result.contains("Hello context world"));
}

#[tokio::test]
async fn test_file_context_missing_file() {
    let tmp = TempDir::new().unwrap();
    let config = ContextConfig {
        sources: vec![ContextSource::File {
            path: "nonexistent.txt".to_string(),
            max_bytes: 50_000,
        }],
        budget_tokens: 0,
        ..default_context_config()
    };

    // Should not error — just warns and skips
    let result = inject_skill_context(&config, tmp.path()).await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn test_budget_truncation() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("big.txt");
    // Create a file larger than 100 chars
    std::fs::write(&file_path, "A".repeat(500)).unwrap();

    let config = ContextConfig {
        sources: vec![ContextSource::File {
            path: "big.txt".to_string(),
            max_bytes: 50_000,
        }],
        budget_tokens: 25, // 25 * 4 = 100 chars budget
        ..default_context_config()
    };

    let result = inject_skill_context(&config, tmp.path()).await.unwrap();
    // The content portion should be at most 100 chars (budget), though the
    // header line adds overhead. The A's should be truncated.
    let a_count = result.chars().filter(|c| *c == 'A').count();
    assert!(a_count <= 100, "Expected at most 100 A's, got {}", a_count);
}

#[tokio::test]
async fn test_glob_context_injection() {
    let tmp = TempDir::new().unwrap();
    let sub = tmp.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("a.rs"), "fn a() {}").unwrap();
    std::fs::write(sub.join("b.rs"), "fn b() {}").unwrap();
    std::fs::write(sub.join("c.txt"), "not rust").unwrap();

    let config = ContextConfig {
        sources: vec![ContextSource::FileGlob {
            pattern: "**/*.rs".to_string(),
            max_files: 10,
            max_bytes_each: 200_000,
        }],
        budget_tokens: 0,
        ..default_context_config()
    };

    let result = inject_skill_context(&config, tmp.path()).await.unwrap();
    assert!(result.contains("fn a()"));
    assert!(result.contains("fn b()"));
    assert!(!result.contains("not rust"));
}

#[test]
fn test_simple_glob_match_exact() {
    assert!(simple_glob_match("foo.rs", "foo.rs"));
    assert!(!simple_glob_match("foo.rs", "bar.rs"));
}

#[test]
fn test_simple_glob_match_star() {
    assert!(simple_glob_match("*.rs", "foo.rs"));
    assert!(simple_glob_match("*.rs", "bar.rs"));
    assert!(!simple_glob_match("*.rs", "foo.txt"));
}

#[test]
fn test_simple_glob_match_doublestar() {
    assert!(simple_glob_match("**/*.rs", "src/main.rs"));
    assert!(simple_glob_match("**/*.rs", "deep/nested/file.rs"));
    assert!(!simple_glob_match("**/*.rs", "file.txt"));
}

#[test]
fn test_glob_max_files_limit() {
    let tmp = TempDir::new().unwrap();
    for i in 0..20 {
        std::fs::write(tmp.path().join(format!("file{}.txt", i)), "content").unwrap();
    }

    let results = find_matching_files(tmp.path(), "*.txt", 5).unwrap();
    assert_eq!(results.len(), 5);
}

#[tokio::test]
async fn test_path_traversal_dotdot_blocked() {
    let tmp = TempDir::new().unwrap();
    // Create a file outside the skill directory
    let parent = tmp.path().parent().unwrap();
    let secret_path = parent.join("secret.txt");
    std::fs::write(&secret_path, "sensitive data").unwrap();

    let config = ContextConfig {
        sources: vec![ContextSource::File {
            path: "../secret.txt".to_string(),
            max_bytes: 50_000,
        }],
        budget_tokens: 0,
        ..default_context_config()
    };

    let result = inject_skill_context(&config, tmp.path()).await.unwrap();
    // Path traversal should be blocked — result should be empty
    assert!(result.is_empty(), "Path traversal should be blocked");
    assert!(
        !result.contains("sensitive data"),
        "Should not read files outside skill directory"
    );

    // Clean up
    let _ = std::fs::remove_file(&secret_path);
}

#[tokio::test]
async fn test_path_within_skill_dir_allowed() {
    let tmp = TempDir::new().unwrap();
    let sub = tmp.path().join("subdir");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("nested.txt"), "nested content").unwrap();

    let config = ContextConfig {
        sources: vec![ContextSource::File {
            path: "subdir/nested.txt".to_string(),
            max_bytes: 50_000,
        }],
        budget_tokens: 0,
        ..default_context_config()
    };

    let result = inject_skill_context(&config, tmp.path()).await.unwrap();
    assert!(
        result.contains("nested content"),
        "Files within skill directory should be readable"
    );
}
