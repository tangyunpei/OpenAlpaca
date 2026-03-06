use super::*;
use base64::Engine as _;
use crate::bus::EventBus;
use crate::tools::registry::BuiltInTool;

/// Helper: create a PersonaUpdateTool backed by a temp directory with a valid SOUL file.
fn make_persona_tool() -> (PersonaUpdateTool, tempfile::TempDir) {
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

    let ctx = PersonaToolContext {
        soul_path,
        user_path: dir.path().join("USER.md"),
        identity_path: dir.path().join("IDENTITY.md"),
        backup_dir: dir.path().join("backups"),
        bus: EventBus::new(16),
        max_backups: None,
    };
    let tool = PersonaUpdateTool {
        soul: soul::SoulHandler {
            soul_path: ctx.soul_path.clone(),
            backup_dir: ctx.backup_dir.clone(),
            bus: ctx.bus.clone(),
            max_backups: ctx.max_backups,
        },
        user: user::UserHandler {
            user_path: ctx.user_path.clone(),
            backup_dir: ctx.backup_dir.clone(),
            bus: ctx.bus.clone(),
            max_backups: ctx.max_backups,
        },
        identity: identity::IdentityHandler {
            identity_path: ctx.identity_path.clone(),
            backup_dir: ctx.backup_dir,
            bus: ctx.bus,
            max_backups: ctx.max_backups,
        },
    };
    (tool, dir)
}

// --- Dispatch tests ---

#[tokio::test]
async fn test_missing_target_returns_error() {
    let (tool, _dir) = make_persona_tool();
    let result = tool
        .execute(&serde_json::json!({"mode": "replace", "content_b64": "x"}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("target"));
}

#[tokio::test]
async fn test_invalid_target_returns_error() {
    let (tool, _dir) = make_persona_tool();
    let result = tool
        .execute(&serde_json::json!({"target": "invalid", "mode": "replace", "content_b64": "x"}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Invalid target"));
}

#[tokio::test]
async fn test_unknown_field_rejected() {
    let (tool, _dir) = make_persona_tool();
    let result = tool
        .execute(&serde_json::json!({"target": "soul", "mode": "replace", "content_b64": "x", "evil_field": true}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unknown field"));
}

// --- Soul replace tests (ported from old update_soul tests) ---

#[tokio::test]
async fn test_soul_replace_valid() {
    let (tool, _dir) = make_persona_tool();
    let valid_soul = "---\ntitle: \"New\"\nsummary: \"New soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe bold.\n\n## Boundaries\n\n- No harm.\n\n## Vibe\n\nPirate style.\n\n## Continuity\n\nRemember.\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
    let result = tool
        .execute(&serde_json::json!({"target": "soul", "mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_ok(), "Valid replace should succeed: {:?}", result);
    let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(json["status"], "applied");
}

#[tokio::test]
async fn test_soul_replace_invalid_base64() {
    let (tool, _dir) = make_persona_tool();
    let result = tool
        .execute(&serde_json::json!({"target": "soul", "mode": "replace", "content_b64": "not-valid-b64!!!"}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Invalid base64"));
}

#[tokio::test]
async fn test_soul_replace_empty_content() {
    let (tool, _dir) = make_persona_tool();
    let b64 = base64::engine::general_purpose::STANDARD.encode("   \n  ");
    let result = tool
        .execute(&serde_json::json!({"target": "soul", "mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("empty"));
}

#[tokio::test]
async fn test_soul_replace_invalid_schema() {
    let (tool, _dir) = make_persona_tool();
    let invalid = "---\ntitle: \"X\"\nsummary: \"X\"\nread_when:\n  - a\n---\n\n## Core Truths\n\nBe good.\n\n## Vibe\n\nChill.\n\n## Continuity\n\nRemember.\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(invalid);
    let result = tool
        .execute(&serde_json::json!({"target": "soul", "mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Boundaries"));
}

#[tokio::test]
async fn test_soul_missing_mode() {
    let (tool, _dir) = make_persona_tool();
    let result = tool
        .execute(&serde_json::json!({"target": "soul", "content_b64": "x"}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("mode"));
}

// --- Soul sections tests ---

#[tokio::test]
async fn test_soul_sections_valid() {
    let (tool, _dir) = make_persona_tool();
    let result = tool
        .execute(&serde_json::json!({
            "target": "soul",
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
async fn test_soul_sections_empty_patch_rejected() {
    let (tool, _dir) = make_persona_tool();
    let result = tool
        .execute(&serde_json::json!({
            "target": "soul",
            "mode": "sections",
            "sections": {}
        }))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("empty"));
}

#[tokio::test]
async fn test_soul_sections_unknown_field_rejected() {
    let (tool, _dir) = make_persona_tool();
    let result = tool
        .execute(&serde_json::json!({
            "target": "soul",
            "mode": "sections",
            "sections": { "evil": "hi" }
        }))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unknown section field"));
}

// --- Backup tests ---

#[tokio::test]
async fn test_soul_creates_backup() {
    let (tool, dir) = make_persona_tool();
    let valid_soul = "---\ntitle: \"New\"\nsummary: \"New soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe bold.\n\n## Boundaries\n\n- No harm.\n\n## Vibe\n\nPirate style.\n\n## Continuity\n\nRemember.\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
    let result = tool
        .execute(&serde_json::json!({"target": "soul", "mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_ok());

    let backup_dir = dir.path().join("backups");
    assert!(backup_dir.exists(), "Backup directory should exist");
    let entries: Vec<_> = std::fs::read_dir(&backup_dir).unwrap().collect();
    assert_eq!(entries.len(), 1, "Should have exactly one backup");

    let backup_path = entries[0].as_ref().unwrap().path();
    let backup_content = std::fs::read_to_string(&backup_path).unwrap();
    assert!(
        backup_content.contains("Test Soul"),
        "Backup should contain original title"
    );
}

#[tokio::test]
async fn test_soul_result_contains_backup_path() {
    let (tool, _dir) = make_persona_tool();
    let valid_soul = "---\ntitle: \"X\"\nsummary: \"X\"\nread_when:\n  - a\n---\n\n## Core Truths\n\nY.\n\n## Boundaries\n\n- Z.\n\n## Vibe\n\nV.\n\n## Continuity\n\nC.\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
    let result = tool
        .execute(&serde_json::json!({"target": "soul", "mode": "replace", "content_b64": b64}))
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
async fn test_soul_validation_failure_does_not_write() {
    let (tool, dir) = make_persona_tool();
    let original = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
    let invalid = "---\ntitle: \"Bad\"\nsummary: \"Bad\"\nread_when:\n  - a\n---\n\n## Core Truths\n\nBe good.\n\n## Vibe\n\nChill.\n\n## Continuity\n\nRemember.\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(invalid);
    let result = tool
        .execute(&serde_json::json!({"target": "soul", "mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_err());

    let after = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
    assert_eq!(
        original, after,
        "Failed validation should not modify SOUL.md"
    );
    assert!(
        !dir.path().join("backups").exists(),
        "No backup for failed validation"
    );
}

#[tokio::test]
async fn test_soul_publishes_event() {
    let (tool, _dir) = make_persona_tool();
    let mut rx = tool.soul.bus.subscribe();

    let valid_soul = "---\ntitle: \"Evented\"\nsummary: \"Event test\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe evented.\n\n## Boundaries\n\n- Stay safe.\n\n## Vibe\n\nEventful.\n\n## Continuity\n\nRemember events.\n";
    let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
    let result = tool
        .execute(&serde_json::json!({"target": "soul", "mode": "replace", "content_b64": b64}))
        .await;
    assert!(result.is_ok());

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

// --- Type validation tests ---

#[tokio::test]
async fn test_sections_rejects_wrong_type_string_fields() {
    let (tool, _dir) = make_persona_tool();

    let result = tool
        .execute(&serde_json::json!({
            "target": "soul",
            "mode": "sections",
            "sections": { "vibe": 123 }
        }))
        .await;
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("must be a string"),
        "vibe=123 should report type error"
    );
}

#[tokio::test]
async fn test_sections_rejects_wrong_type_array_fields() {
    let (tool, _dir) = make_persona_tool();

    let result = tool
        .execute(&serde_json::json!({
            "target": "soul",
            "mode": "sections",
            "sections": { "core_truths": "not array" }
        }))
        .await;
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("must be an array"),
        "core_truths=string should report type error"
    );
}

// --- User identity as JSON string (OpenAI strict mode compatibility) ---

/// Helper: write a minimal valid USER.md to the tool's user_path.
fn write_valid_user_md(tool: &PersonaUpdateTool) {
    let valid_user = r#"---
title: "Test User"
summary: "A test user profile"
read_when:
  - always
---

## Identity

* Name: Test
* Timezone: UTC

## Communication Style

Concise.

## Expertise & Background

Rust.

## Projects & Context

OpenAlpaca.

## Preferences

None.

## Notes

None.
"#;
    std::fs::write(&tool.user.user_path, valid_user).unwrap();
}

#[tokio::test]
async fn test_user_sections_identity_as_json_string() {
    let (tool, _dir) = make_persona_tool();
    write_valid_user_md(&tool);

    let result = tool
        .execute(&serde_json::json!({
            "target": "user",
            "mode": "sections",
            "sections": { "identity": "{\"Name\": \"Alice\", \"role\": \"engineer\"}" }
        }))
        .await;
    assert!(
        result.is_ok(),
        "JSON-encoded identity string should be accepted: {:?}",
        result
    );
    let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(json["status"], "applied");
}

#[tokio::test]
async fn test_user_sections_identity_as_object() {
    let (tool, _dir) = make_persona_tool();
    write_valid_user_md(&tool);

    let result = tool
        .execute(&serde_json::json!({
            "target": "user",
            "mode": "sections",
            "sections": { "identity": {"Name": "Bob"} }
        }))
        .await;
    assert!(
        result.is_ok(),
        "Object identity should still be accepted: {:?}",
        result
    );
}

#[tokio::test]
async fn test_user_sections_identity_invalid_json_string() {
    let (tool, _dir) = make_persona_tool();
    write_valid_user_md(&tool);

    let result = tool
        .execute(&serde_json::json!({
            "target": "user",
            "mode": "sections",
            "sections": { "identity": "not valid json" }
        }))
        .await;
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("not valid JSON"),
        "Invalid JSON string should be rejected"
    );
}
