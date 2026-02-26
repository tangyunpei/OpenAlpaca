use super::*;
use crate::bus::EventBus;

/// Helper: create a SoulUpdateTool backed by a temp directory with a valid SOUL file.
fn make_soul_tool() -> (SoulUpdateTool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let soul_path = dir.path().join("SOUL.md");
    let valid_soul = r#"---
title: "Test Soul"
summary: "A test soul"
read_when:
  - always
---

## Core Truths

Be helpful.

## Boundaries

- Stay safe.

## Vibe

Friendly and clear.

## Continuity

Remember everything.
"#;
    std::fs::write(&soul_path, valid_soul).unwrap();

    let ctx = SoulToolContext {
        soul_path,
        backup_dir: dir.path().join("backups"),
        bus: EventBus::new(16),
        max_backups: None,
    };
    (SoulUpdateTool { ctx }, dir)
}

#[tokio::test]
async fn test_soul_update_replace_valid() {
    let (tool, _dir) = make_soul_tool();
    let valid_soul = "---\ntitle: \"New\"\nsummary: \"New soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe bold.\n\n## Boundaries\n\n- No harm.\n\n## Vibe\n\nPirate style.\n\n## Continuity\n\nRemember.\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
    let result = tool
        .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_ok(), "Valid replace should succeed: {:?}", result);
    let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(json["status"], "applied");
}

#[tokio::test]
async fn test_soul_update_replace_invalid_base64() {
    let (tool, _dir) = make_soul_tool();
    let result = tool
        .execute(&serde_json::json!({"mode": "replace", "content_b64": "not-valid-b64!!!"}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Invalid base64"));
}

#[tokio::test]
async fn test_soul_update_replace_empty_content() {
    let (tool, _dir) = make_soul_tool();
    // Base64 of just whitespace
    let b64 = base64::engine::general_purpose::STANDARD.encode("   \n  ");
    let result = tool
        .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("empty"));
}

#[tokio::test]
async fn test_soul_update_replace_invalid_schema() {
    let (tool, _dir) = make_soul_tool();
    // Missing ## Boundaries section
    let invalid = "---\ntitle: \"X\"\nsummary: \"X\"\nread_when:\n  - a\n---\n\n## Core Truths\n\nBe good.\n\n## Vibe\n\nChill.\n\n## Continuity\n\nRemember.\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(invalid);
    let result = tool
        .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Boundaries"));
}

#[tokio::test]
async fn test_soul_update_unknown_field_rejected() {
    let (tool, _dir) = make_soul_tool();
    let result = tool
        .execute(&serde_json::json!({"mode": "replace", "content_b64": "x", "evil_field": true}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unknown field"));
}

#[tokio::test]
async fn test_soul_update_missing_mode() {
    let (tool, _dir) = make_soul_tool();
    let result = tool.execute(&serde_json::json!({"content_b64": "x"})).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("mode"));
}

#[tokio::test]
async fn test_soul_update_sections_valid() {
    let (tool, _dir) = make_soul_tool();
    let result = tool
        .execute(&serde_json::json!({
            "mode": "sections",
            "sections": { "vibe": "Pirate style, arr!" }
        }))
        .await;
    assert!(
        result.is_ok(),
        "Valid sections patch should succeed: {:?}",
        result
    );
    let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(json["status"], "applied");
}

#[tokio::test]
async fn test_soul_update_sections_empty_patch_rejected() {
    let (tool, _dir) = make_soul_tool();
    let result = tool
        .execute(&serde_json::json!({
            "mode": "sections",
            "sections": {}
        }))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("empty"));
}

#[tokio::test]
async fn test_soul_update_sections_unknown_field_rejected() {
    let (tool, _dir) = make_soul_tool();
    let result = tool
        .execute(&serde_json::json!({
            "mode": "sections",
            "sections": { "evil": "hi" }
        }))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unknown section field"));
}

#[tokio::test]
async fn test_soul_update_replace_empty_b64_rejected() {
    let (tool, _dir) = make_soul_tool();
    let result = tool
        .execute(&serde_json::json!({"mode": "replace", "content_b64": ""}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("empty"));
}

#[tokio::test]
async fn test_soul_update_creates_backup() {
    let (tool, dir) = make_soul_tool();
    let valid_soul = "---\ntitle: \"New\"\nsummary: \"New soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe bold.\n\n## Boundaries\n\n- No harm.\n\n## Vibe\n\nPirate style.\n\n## Continuity\n\nRemember.\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
    let result = tool
        .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_ok());

    // Verify backup directory was created and contains a backup
    let backup_dir = dir.path().join("backups");
    assert!(backup_dir.exists(), "Backup directory should exist");
    let entries: Vec<_> = std::fs::read_dir(&backup_dir).unwrap().collect();
    assert_eq!(entries.len(), 1, "Should have exactly one backup");

    // Verify backup content matches original, not the new content
    let backup_path = entries[0].as_ref().unwrap().path();
    let backup_content = std::fs::read_to_string(&backup_path).unwrap();
    assert!(
        backup_content.contains("Test Soul"),
        "Backup should contain original title"
    );
}

#[tokio::test]
async fn test_soul_update_atomic_write_applies_new_content() {
    let (tool, dir) = make_soul_tool();
    let valid_soul = "---\ntitle: \"Updated\"\nsummary: \"Updated soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe bold.\n\n## Boundaries\n\n- No harm.\n\n## Vibe\n\nPirate style.\n\n## Continuity\n\nRemember.\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
    let result = tool
        .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_ok());

    // Verify the SOUL file now has the new content
    let current = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
    assert!(
        current.contains("Updated"),
        "SOUL.md should have new content"
    );
    // Verify no temp file remains
    assert!(
        !dir.path().join(".SOUL.md.tmp").exists(),
        "Temp file should not remain"
    );
}

#[tokio::test]
async fn test_soul_update_result_contains_backup_path() {
    let (tool, _dir) = make_soul_tool();
    let valid_soul = "---\ntitle: \"X\"\nsummary: \"X\"\nread_when:\n  - a\n---\n\n## Core Truths\n\nY.\n\n## Boundaries\n\n- Z.\n\n## Vibe\n\nV.\n\n## Continuity\n\nC.\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
    let result = tool
        .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_ok());
    let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert!(
        json["backup_path"].is_string(),
        "Result should contain backup_path"
    );
    assert!(
        json["backup_path"].as_str().unwrap().contains("SOUL."),
        "Backup path should contain timestamped name"
    );
}

#[tokio::test]
async fn test_soul_update_validation_failure_does_not_write() {
    let (tool, dir) = make_soul_tool();
    let original = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
    // Invalid SOUL - missing Boundaries
    let invalid = "---\ntitle: \"Bad\"\nsummary: \"Bad\"\nread_when:\n  - a\n---\n\n## Core Truths\n\nBe good.\n\n## Vibe\n\nChill.\n\n## Continuity\n\nRemember.\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(invalid);
    let result = tool
        .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_err());

    // Original file should be untouched
    let after = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
    assert_eq!(
        original, after,
        "Failed validation should not modify SOUL.md"
    );
    // No backup should be created for failed validation
    assert!(
        !dir.path().join("backups").exists(),
        "No backup for failed validation"
    );
}

#[tokio::test]
async fn test_soul_update_publishes_soul_updated_event() {
    let (tool, _dir) = make_soul_tool();

    // Subscribe BEFORE executing the tool
    let mut rx = tool.ctx.bus.subscribe();

    let valid_soul = "---\ntitle: \"Evented\"\nsummary: \"Event test\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe evented.\n\n## Boundaries\n\n- Stay safe.\n\n## Vibe\n\nEventful.\n\n## Continuity\n\nRemember events.\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
    let result = tool
        .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_ok());

    // Verify the SoulUpdated event was published
    let event = rx
        .try_recv()
        .expect("Should have received SoulUpdated event");
    match event {
        crate::events::SystemEvent::SoulUpdated {
            actor,
            mode,
            content_sha256,
            ..
        } => {
            assert_eq!(actor, "agent");
            assert_eq!(mode, "replace");
            assert!(!content_sha256.is_empty(), "Hash should not be empty");
        }
        other => panic!("Expected SoulUpdated, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_soul_update_with_max_backups_prunes() {
    let dir = tempfile::tempdir().unwrap();
    let soul_path = dir.path().join("SOUL.md");
    let backup_dir = dir.path().join("backups");
    std::fs::create_dir_all(&backup_dir).unwrap();

    let valid_soul = "---\ntitle: \"Test\"\nsummary: \"Test soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe helpful.\n\n## Boundaries\n\n- Stay safe.\n\n## Vibe\n\nFriendly.\n\n## Continuity\n\nRemember.\n";
    std::fs::write(&soul_path, valid_soul).unwrap();

    // Pre-create 3 old backups
    for i in 1..=3 {
        std::fs::write(
            backup_dir.join(format!("SOUL.20250101T00000{}Z.md", i)),
            format!("old {}", i),
        )
        .unwrap();
    }

    let ctx = SoulToolContext {
        soul_path,
        backup_dir: backup_dir.clone(),
        bus: EventBus::new(16),
        max_backups: Some(2), // Keep only 2 backups
    };
    let tool = SoulUpdateTool { ctx };

    let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
    let result = tool
        .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_ok());

    // After creating 1 new backup + 3 old = 4 total, pruned to 2
    let count = std::fs::read_dir(&backup_dir)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .ok()
                .and_then(|e| e.file_name().to_str().map(|n| n.starts_with("SOUL.")))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(count, 2, "Should have pruned to max_backups=2");
}

#[tokio::test]
async fn test_sections_rejects_wrong_type_string_fields() {
    let (tool, _dir) = make_soul_tool();

    // vibe: number instead of string
    let result = tool
        .execute(&serde_json::json!({
            "mode": "sections",
            "sections": { "vibe": 123 }
        }))
        .await;
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("must be a string"),
        "vibe=123 should report type error"
    );

    // summary: bool instead of string
    let result = tool
        .execute(&serde_json::json!({
            "mode": "sections",
            "sections": { "summary": true }
        }))
        .await;
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("must be a string"),
        "summary=true should report type error"
    );

    // title: array instead of string
    let result = tool
        .execute(&serde_json::json!({
            "mode": "sections",
            "sections": { "title": ["array"] }
        }))
        .await;
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("must be a string"),
        "title=[array] should report type error"
    );
}

#[tokio::test]
async fn test_sections_rejects_wrong_type_array_fields() {
    let (tool, _dir) = make_soul_tool();

    // core_truths: string instead of array
    let result = tool
        .execute(&serde_json::json!({
            "mode": "sections",
            "sections": { "core_truths": "not array" }
        }))
        .await;
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("must be an array"),
        "core_truths=string should report type error"
    );

    // boundaries: object instead of array
    let result = tool
        .execute(&serde_json::json!({
            "mode": "sections",
            "sections": { "boundaries": {"obj": true} }
        }))
        .await;
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("must be an array"),
        "boundaries=object should report type error"
    );

    // continuity: number instead of array
    let result = tool
        .execute(&serde_json::json!({
            "mode": "sections",
            "sections": { "continuity": 42 }
        }))
        .await;
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("must be an array"),
        "continuity=42 should report type error"
    );
}

#[tokio::test]
async fn test_wrong_type_does_not_mutate_soul_or_create_backup() {
    let (tool, dir) = make_soul_tool();
    let original = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();

    // Attempt with wrong type for vibe
    let result = tool
        .execute(&serde_json::json!({
            "mode": "sections",
            "sections": { "vibe": 999 }
        }))
        .await;
    assert!(result.is_err());

    // File should be unchanged
    let after = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
    assert_eq!(
        original, after,
        "SOUL.md should not be modified on type error"
    );

    // No backup directory should be created
    assert!(
        !dir.path().join("backups").exists(),
        "No backup dir should exist for failed type validation"
    );
}

#[tokio::test]
async fn test_rapid_updates_produce_distinct_backups() {
    let dir = tempfile::tempdir().unwrap();
    let soul_path = dir.path().join("SOUL.md");
    let backup_dir = dir.path().join("backups");

    let valid_soul = "---\ntitle: \"Test\"\nsummary: \"Test soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe helpful.\n\n## Boundaries\n\n- Stay safe.\n\n## Vibe\n\nFriendly.\n\n## Continuity\n\nRemember.\n";
    std::fs::write(&soul_path, valid_soul).unwrap();

    let ctx = SoulToolContext {
        soul_path,
        backup_dir: backup_dir.clone(),
        bus: EventBus::new(16),
        max_backups: None,
    };
    let tool = SoulUpdateTool { ctx };

    let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);

    // Perform 5 rapid updates
    for _ in 0..5 {
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_ok(), "Update should succeed: {:?}", result);
    }

    // Count backup files
    let backup_count = std::fs::read_dir(&backup_dir)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .ok()
                .and_then(|e| {
                    e.file_name()
                        .to_str()
                        .map(|n| n.starts_with("SOUL.") && n.ends_with(".md"))
                })
                .unwrap_or(false)
        })
        .count();

    assert_eq!(
        backup_count, 5,
        "Should have 5 distinct backup files, no overwrites"
    );
}
