//! `memory_files` unit tests: the key derivations (design §2), the note canon
//! (§4), the snapshot (§7a), and — as the partition lands — confinement,
//! modes, `.history`, the mutation protocol with a crash at every step (§6),
//! derived supersession and the management operations.
//!
//! Nothing here touches a real root: every partition lives under a temp
//! directory, and the home store is pointed at one through
//! `OPENALPACA_HOME_STORE` for the duration of the test.

use super::*;
use chrono::{TimeZone, Utc};

// ============================================================================
// Keys (design §2; the literal values are the fixtures README's)
// ============================================================================

#[test]
fn owner_key_is_first_32_hex_of_sha256_over_utf8() {
    assert_eq!(
        owner_key("u_example_alice"),
        "32acea7eb739b08afd959db91e46d450"
    );
    assert_eq!(
        owner_key("u_example_bob"),
        "5c357145f3a583c080c9f1d8ee520e71"
    );
    assert_eq!(owner_key("system"), "bbc5e661e106c6dcd8dc6dd186454c2f");
    assert_eq!(
        owner_key("u_example_alice_reborn"),
        "9bfb77d3249642b6ba22fe7dcade07a0"
    );
    assert_eq!(owner_key("").len(), OWNER_KEY_HEX);
    assert_eq!(owner_key("用户").len(), OWNER_KEY_HEX);
}

#[test]
fn partition_keys_have_the_design_shape() {
    let owner = owner_key("u_example_alice");
    assert_eq!(
        home_partition_key(&owner),
        "home-32acea7eb739b08afd959db91e46d450"
    );
    let dir = "/tmp/x/.openalpaca/memory/owners/32acea7eb739b08afd959db91e46d450";
    let key = project_partition_key(&owner, dir);
    assert!(key.starts_with("proj-32acea7eb739b08afd959db91e46d450-"));
    assert_eq!(key.len(), "proj-".len() + OWNER_KEY_HEX + 1 + DIR_KEY_HEX);
    assert_eq!(&key["proj-".len() + OWNER_KEY_HEX + 1..], dir_key(dir));
    assert_eq!(dir_key(dir), &sha256_hex(dir.as_bytes())[..DIR_KEY_HEX]);
    // Two different directories, two different keys; the same one, the same.
    assert_ne!(dir_key(dir), dir_key("/tmp/y/.openalpaca/memory/owners/abc"));
    assert_eq!(project_partition_key(&owner, dir), key);
}

#[test]
fn revision_is_lowercase_hex_sha256_of_exact_bytes() {
    let rev = revision_of(b"abc");
    assert_eq!(
        rev,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert!(is_revision(&rev));
    assert!(!is_revision(&rev.to_uppercase()));
    assert!(!is_revision("abc"));
}

// ============================================================================
// The note canon (design §4)
// ============================================================================

/// The design §4 example, verbatim.
const EXAMPLE: &str = "---
schema: 1
id: 55e811b6-3220-4758-a47b-034bd1023f38
title: Build and test commands for this workspace
kind: procedure
status: active
fact_key: build.validation
aliases:
  - checks
  - validation
  - 测试
  - 编译
tags:
  - development
created_at: 2026-09-23T10:00:00Z
updated_at: 2026-09-23T10:00:00Z
observed_at: 2026-09-23T09:55:00Z
valid_from: null
valid_to: null
review_after: null
pinned: false
sources:
  - type: task
    id: 7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90
    observed_at: 2026-09-23T09:55:00Z
supersedes: []
---
# Build and test commands for this workspace

## Knowledge
Run the documented focused checks for the component being changed.

## Evidence and limits
The cited run checked one component. It did not certify the entire application.

## Recheck when
The build configuration or test entry points change.
";

fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
}

#[test]
fn design_example_round_trips_byte_identically() {
    let note = Note::parse(EXAMPLE.as_bytes()).expect("the design example parses");
    assert_eq!(note.id.to_string(), "55e811b6-3220-4758-a47b-034bd1023f38");
    assert_eq!(note.title, "Build and test commands for this workspace");
    assert_eq!(note.kind, NoteKind::Procedure);
    assert_eq!(note.status, NoteStatus::Active);
    assert_eq!(note.fact_key.as_deref(), Some("build.validation"));
    assert_eq!(note.aliases, ["checks", "validation", "测试", "编译"]);
    assert_eq!(note.tags, ["development"]);
    assert_eq!(note.created_at, ts(2026, 9, 23, 10, 0, 0));
    assert_eq!(note.observed_at, Some(ts(2026, 9, 23, 9, 55, 0)));
    assert_eq!(note.valid_from, None);
    assert!(!note.pinned);
    assert_eq!(note.sources.len(), 1);
    assert_eq!(note.sources[0].kind, SourceType::Task);
    assert_eq!(note.sources[0].id, "7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90");
    assert_eq!(
        note.sources[0].observed_at,
        Some(ts(2026, 9, 23, 9, 55, 0))
    );
    assert!(note.supersedes.is_empty());
    assert!(note.unknown.is_empty());
    assert!(note.body.starts_with("# Build and test commands"));
    assert_eq!(note.serialize(), EXAMPLE, "parse then write is byte-identical");
    assert_eq!(note.revision(), revision_of(EXAMPLE.as_bytes()));
    assert_eq!(
        note.file_name(),
        "55e811b6-3220-4758-a47b-034bd1023f38--build-and-test-commands-for-this-workspace.md"
    );
}

#[test]
fn unknown_keys_are_preserved_verbatim_after_the_known_ones() {
    let text = "---
schema: 1
id: 55e811b6-3220-4758-a47b-034bd1023f38
title: T
kind: fact
status: active
fact_key: null
aliases: []
tags: []
created_at: 2026-09-23T10:00:00Z
updated_at: 2026-09-23T10:00:00Z
observed_at: null
valid_from: null
valid_to: null
review_after: null
pinned: false
sources: []
supersedes: []
x_custom: some value
x_block:
  - one
  - two
x_map:
  nested: 1
  other: [a, b]
---
body
";
    let note = Note::parse(text.as_bytes()).unwrap();
    assert_eq!(
        note.unknown
            .iter()
            .map(|u| u.key.as_str())
            .collect::<Vec<_>>(),
        ["x_custom", "x_block", "x_map"]
    );
    assert_eq!(note.unknown[1].raw, "x_block:\n  - one\n  - two\n");
    assert_eq!(note.serialize(), text);
}

#[test]
fn unknown_keys_written_before_known_ones_move_after_them_once() {
    let text = "---
x_first: 1
schema: 1
id: 55e811b6-3220-4758-a47b-034bd1023f38
title: T
kind: fact
status: active
created_at: 2026-09-23T10:00:00Z
updated_at: 2026-09-23T10:00:00Z
---
body
";
    let note = Note::parse(text.as_bytes()).unwrap();
    let canon = note.serialize();
    assert!(canon.ends_with("supersedes: []\nx_first: 1\n---\nbody\n"));
    // The second pass is stable.
    let again = Note::parse(canon.as_bytes()).unwrap();
    assert_eq!(again.serialize(), canon);
    assert_eq!(again, note);
}

#[test]
fn cjk_aliases_and_title_round_trip() {
    let mut note = Note::parse(EXAMPLE.as_bytes()).unwrap();
    note.title = "构建与测试命令 — für die Werkstatt".to_string();
    note.aliases = vec!["测试".into(), "编译".into(), "🚀 launch".into()];
    note.body = "# 标题\n\n正文。\n".to_string();
    let canon = note.serialize();
    let again = Note::parse(canon.as_bytes()).unwrap();
    assert_eq!(again, note);
    assert_eq!(again.serialize(), canon);
    assert!(canon.contains("title: 构建与测试命令 — für die Werkstatt\n"));
    assert!(canon.contains("  - 🚀 launch\n"));
}

#[test]
fn comments_are_dropped_and_the_result_is_stable() {
    let text = "---
# a comment before everything
schema: 1
id: 55e811b6-3220-4758-a47b-034bd1023f38
title: T # trailing comment
kind: fact
status: active
aliases:
  # inside a block
  - a
created_at: 2026-09-23T10:00:00Z
updated_at: 2026-09-23T10:00:00Z
---
body # not a comment: the body is bytes
";
    let note = Note::parse(text.as_bytes()).unwrap();
    assert_eq!(note.title, "T");
    assert_eq!(note.aliases, ["a"]);
    let canon = note.serialize();
    assert!(!canon.contains("comment before"));
    assert!(!canon.contains("inside a block"));
    assert!(canon.ends_with("---\nbody # not a comment: the body is bytes\n"));
    assert_eq!(Note::parse(canon.as_bytes()).unwrap().serialize(), canon);
}

#[test]
fn scalars_that_yaml_would_misread_are_quoted_and_read_back() {
    let mut note = Note::parse(EXAMPLE.as_bytes()).unwrap();
    for title in [
        "null",
        "- leading dash",
        "key: value",
        "a # hash",
        "  padded  ",
        "\"quoted\"",
        "tab\there",
        "multi\nline",
        "[flow]",
        "@at",
        "../../../etc/passwd",
        "42",
        "true",
    ] {
        note.title = title.to_string();
        note.aliases = vec![title.to_string()];
        let canon = note.serialize();
        let again = Note::parse(canon.as_bytes())
            .unwrap_or_else(|e| panic!("{title:?}: {e}"));
        assert_eq!(again.title, title, "title {title:?}");
        assert_eq!(again.aliases, [title], "alias {title:?}");
        assert_eq!(again.serialize(), canon);
    }
    // Plain where YAML reads it back unchanged.
    note.title = "42".into();
    assert!(note.serialize().contains("title: 42\n"));
    note.title = "../../../etc/passwd".into();
    assert!(note.serialize().contains("title: ../../../etc/passwd\n"));
}

#[test]
fn single_quoted_and_flow_lists_are_read() {
    let text = "---
schema: 1
id: 55e811b6-3220-4758-a47b-034bd1023f38
title: 'it''s quoted'
kind: fact
status: active
aliases: [a, \"b, c\", 'd']
tags:
- no-indent
- item
created_at: 2026-09-23T10:00:00+02:00
updated_at: 2026-09-23T10:00:00.5Z
pinned: True
---
";
    let note = Note::parse(text.as_bytes()).unwrap();
    assert_eq!(note.title, "it's quoted");
    assert_eq!(note.aliases, ["a", "b, c", "d"]);
    assert_eq!(note.tags, ["no-indent", "item"]);
    assert_eq!(note.created_at, ts(2026, 9, 23, 8, 0, 0), "offset normalized to UTC");
    assert!(note.pinned);
    assert_eq!(note.body, "");
    let canon = note.serialize();
    assert!(canon.contains("created_at: 2026-09-23T08:00:00Z\n"));
    assert!(canon.contains("updated_at: 2026-09-23T10:00:00Z\n"), "sub-seconds dropped");
}

#[test]
fn malformed_files_are_refused_with_a_reason() {
    let base = |patch: &dyn Fn(&mut String)| {
        let mut s = EXAMPLE.to_string();
        patch(&mut s);
        s
    };
    let cases: Vec<(&str, String)> = vec![
        ("no frontmatter", "# just markdown\n".to_string()),
        ("no closing", "---\nschema: 1\nid: x\n".to_string()),
        (
            "missing id",
            base(&|s| *s = s.replace("id: 55e811b6-3220-4758-a47b-034bd1023f38\n", "")),
        ),
        (
            "path id",
            base(&|s| {
                *s = s.replace(
                    "id: 55e811b6-3220-4758-a47b-034bd1023f38",
                    "id: ../../state/memory-control/home.json",
                )
            }),
        ),
        (
            "uppercase id",
            base(&|s| {
                *s = s.replace(
                    "id: 55e811b6-3220-4758-a47b-034bd1023f38",
                    "id: 55E811B6-3220-4758-A47B-034BD1023F38",
                )
            }),
        ),
        (
            "duplicate key",
            base(&|s| *s = s.replace("kind: procedure\n", "kind: procedure\nkind: fact\n")),
        ),
        ("bad kind", base(&|s| *s = s.replace("kind: procedure", "kind: memo"))),
        ("bad status", base(&|s| *s = s.replace("status: active", "status: superseded"))),
        (
            "bad timestamp",
            base(&|s| *s = s.replace("created_at: 2026-09-23T10:00:00Z", "created_at: yesterday")),
        ),
        ("bad schema", base(&|s| *s = s.replace("schema: 1", "schema: 2"))),
        (
            "bad supersedes",
            base(&|s| *s = s.replace("supersedes: []", "supersedes:\n  - not-a-uuid")),
        ),
        ("empty title", base(&|s| *s = s.replace("title: Build and test commands for this workspace", "title: \"\""))),
        (
            "unknown source field",
            base(&|s| *s = s.replace("    id: 7c1e0b2a", "    who: me\n    id: 7c1e0b2a")),
        ),
        ("not utf8", String::from_utf8_lossy(&[0xff, 0xfe]).into_owned()),
    ];
    for (name, text) in cases {
        let bytes = if name == "not utf8" {
            vec![b'-', b'-', b'-', b'\n', 0xff, 0xfe]
        } else {
            text.into_bytes()
        };
        let result = Note::parse(&bytes);
        assert!(result.is_err(), "{name} must be malformed");
        let reason = result.unwrap_err().reason;
        assert!(!reason.is_empty(), "{name} names a reason");
    }
}

#[test]
fn oversized_files_are_refused_at_parse_and_at_write() {
    let mut note = Note::parse(EXAMPLE.as_bytes()).unwrap();
    note.body = "x".repeat(MAX_NOTE_BYTES + 1);
    let bytes = note.serialize();
    let e = Note::parse(bytes.as_bytes()).unwrap_err();
    assert!(e.reason.contains("oversized"), "{e}");
    let e = note.validate_for_write().unwrap_err();
    assert!(e.contains("32"), "{e}");
    // Exactly at the limit is fine.
    let header = note.serialize().len() - note.body.len();
    note.body = "x".repeat(MAX_NOTE_BYTES - header);
    assert_eq!(note.serialize().len(), MAX_NOTE_BYTES);
    assert!(note.validate_for_write().is_ok());
    assert!(Note::parse(note.serialize().as_bytes()).is_ok());
    note.body.push('x');
    assert!(note.validate_for_write().is_err());
}

#[test]
fn invalid_fact_key_in_a_file_is_absent_and_refused_on_write() {
    let text = EXAMPLE.replace("fact_key: build.validation", "fact_key: Build Validation!");
    let note = Note::parse(text.as_bytes()).unwrap();
    assert_eq!(note.fact_key, None, "treated as absent when found in a file");
    let mut note = Note::parse(EXAMPLE.as_bytes()).unwrap();
    note.fact_key = Some("Build".into());
    assert!(note.validate_for_write().unwrap_err().contains("fact_key"));
    for ok in ["a", "build.validation", "a-b_c.d", &"a".repeat(64)] {
        assert!(is_valid_fact_key(ok), "{ok}");
    }
    for bad in ["", ".a", "-a", "A", "a b", &"a".repeat(65), "a/b"] {
        assert!(!is_valid_fact_key(bad), "{bad}");
    }
}

#[test]
fn write_limits_are_enforced() {
    let mut note = Note::parse(EXAMPLE.as_bytes()).unwrap();
    note.title = "t".repeat(MAX_TITLE_CHARS + 1);
    assert!(note.validate_for_write().unwrap_err().contains("title"));
    note.title = "ok".into();
    note.aliases = vec!["a".into(); MAX_LIST_ENTRIES + 1];
    assert!(note.validate_for_write().unwrap_err().contains("aliases"));
    note.aliases.clear();
    note.sources = vec![note.sources[0].clone(); MAX_SOURCES + 1];
    assert!(note.validate_for_write().unwrap_err().contains("sources"));
}

#[test]
fn file_names_follow_the_grammar() {
    let id = NoteId::parse("55e811b6-3220-4758-a47b-034bd1023f38").unwrap();
    assert_eq!(
        Note::file_name_for(id, "../../../etc/passwd"),
        "55e811b6-3220-4758-a47b-034bd1023f38--etc-passwd.md"
    );
    assert_eq!(
        Note::file_name_for(id, "   "),
        "55e811b6-3220-4758-a47b-034bd1023f38--note.md"
    );
    assert_eq!(
        Note::file_name_for(id, "测试"),
        "55e811b6-3220-4758-a47b-034bd1023f38--note.md"
    );
    let long = Note::file_name_for(id, &"word ".repeat(40));
    let slug = long
        .trim_end_matches(".md")
        .split_once("--")
        .unwrap()
        .1
        .to_string();
    assert!(slug.len() <= SLUG_MAX_BYTES, "{slug}");
    assert_eq!(Note::id_from_file_name(&long), Some(id));
    assert_eq!(Note::id_from_file_name("readme.md"), None);
    assert_eq!(Note::id_from_file_name("not-a-uuid--x.md"), None);
}

#[test]
fn note_ids_are_canonical_only() {
    assert!(NoteId::parse("55e811b6-3220-4758-a47b-034bd1023f38").is_ok());
    assert!(NoteId::parse("55E811B6-3220-4758-A47B-034BD1023F38").is_err());
    assert!(NoteId::parse("55e811b632204758a47b034bd1023f38").is_err());
    assert!(NoteId::parse("../x").is_err());
    let fresh = NoteId::new();
    assert_eq!(NoteId::parse(&fresh.to_string()).unwrap(), fresh);
}

#[test]
fn kinds_default_scopes_per_rule_2() {
    assert_eq!(NoteKind::Preference.default_scope(), NoteScope::Global);
    for k in [
        NoteKind::Fact,
        NoteKind::Decision,
        NoteKind::Procedure,
        NoteKind::Reference,
        NoteKind::Episode,
    ] {
        assert_eq!(k.default_scope(), NoteScope::Project, "{k}");
    }
}

#[test]
fn a_patch_touches_only_what_it_names() {
    let note = Note::parse(EXAMPLE.as_bytes()).unwrap();
    let mut patched = note.clone();
    NotePatch {
        aliases: Some(vec!["vi".into()]),
        ..Default::default()
    }
    .apply(&mut patched);
    assert_eq!(patched.aliases, ["vi"]);
    assert_eq!(patched.body, note.body);
    assert_eq!(patched.title, note.title);
    let mut cleared = note.clone();
    NotePatch {
        fact_key: Some(None),
        ..Default::default()
    }
    .apply(&mut cleared);
    assert_eq!(cleared.fact_key, None);
    assert!(NotePatch::default().is_empty());
}

// ============================================================================
// The control snapshot (design §7a)
// ============================================================================

/// The design §7a example, verbatim.
const SNAPSHOT_EXAMPLE: &str = r#"{
  "schema": 1,
  "partition_key": "proj-3f9a1c2d4e5b6a7f8091a2b3c4d5e6f7-0a1b2c3d4e5f6071",
  "scope": "project",
  "owner_key": "3f9a1c2d4e5b6a7f8091a2b3c4d5e6f7",
  "owner_id": "u_01J8Z3Q9K7X2M4N6P8R0S2T4V6",
  "root": "/Users/x/dev/app/.openalpaca/memory/owners/3f9a1c2d4e5b6a7f8091a2b3c4d5e6f7",
  "created_at": "2026-09-23T10:00:00Z",
  "updated_at": "2026-09-23T10:05:00Z",
  "moved_from": null,
  "repaired_at": null,
  "pending": null,
  "notes": {
    "55e811b6-3220-4758-a47b-034bd1023f38": {
      "revision": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
      "origin": "extracted_user",
      "written_at": "2026-09-23T10:05:00Z"
    }
  },
  "tombstones": [
    {
      "id": "0c7b2e4a-9d13-4c8e-b1f5-6a2d3e4f5a6b",
      "origin": "user",
      "supersedes": ["d4e5f6a7-b8c9-4d0e-9f1a-2b3c4d5e6f70"],
      "suppressed_sources": ["message:4812", "task:7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90"],
      "forgotten_at": "2026-09-23T10:02:00Z"
    }
  ]
}
"#;

#[test]
fn snapshot_example_parses_and_reserializes_with_the_design_keys_in_order() {
    let snapshot: ControlSnapshot = serde_json::from_str(SNAPSHOT_EXAMPLE).unwrap();
    assert_eq!(snapshot.schema, SNAPSHOT_SCHEMA);
    assert_eq!(snapshot.scope, SnapshotScope::Project);
    assert_eq!(snapshot.notes.len(), 1);
    let id = NoteId::parse("55e811b6-3220-4758-a47b-034bd1023f38").unwrap();
    assert_eq!(snapshot.notes[&id].origin, Origin::ExtractedUser);
    assert_eq!(snapshot.tombstones[0].suppressed_sources.len(), 2);
    assert!(snapshot.is_source_suppressed("message:4812"));
    assert!(!snapshot.is_source_suppressed("message:4813"));
    let json = snapshot.to_json();
    let again: ControlSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(again, snapshot);
    let keys: Vec<&str> = json
        .lines()
        .filter(|l| l.starts_with("  \""))
        .map(|l| l.trim().split('"').nth(1).unwrap())
        .collect();
    assert_eq!(
        keys,
        [
            "schema",
            "partition_key",
            "scope",
            "owner_key",
            "owner_id",
            "root",
            "created_at",
            "updated_at",
            "moved_from",
            "repaired_at",
            "pending",
            "notes",
            "tombstones"
        ]
    );
}

#[test]
fn snapshot_unknown_fields_and_schemas_are_invalid() {
    let with_extra = SNAPSHOT_EXAMPLE.replace("\"schema\": 1,", "\"schema\": 1,\n  \"extra\": true,");
    assert!(serde_json::from_str::<ControlSnapshot>(&with_extra).is_err());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.json");
    std::fs::write(&path, SNAPSHOT_EXAMPLE.replace("\"schema\": 1", "\"schema\": 7")).unwrap();
    assert!(matches!(
        ControlSnapshot::load(&path),
        Err(SnapshotError::Invalid(_))
    ));
    std::fs::write(&path, "{ truncated").unwrap();
    assert!(matches!(
        ControlSnapshot::load(&path),
        Err(SnapshotError::Invalid(_))
    ));
    std::fs::remove_file(&path).unwrap();
    assert!(ControlSnapshot::load(&path).unwrap().is_none());
}

#[test]
fn snapshot_save_is_atomic_private_and_compacts() {
    let dir = tempfile::tempdir().unwrap();
    let control = dir.path().join("memory-control");
    let path = control.join("home-abc.json");
    let mut snapshot: ControlSnapshot = serde_json::from_str(SNAPSHOT_EXAMPLE).unwrap();
    let dup = snapshot.tombstones[0].clone();
    snapshot.tombstones.push(dup);
    snapshot.tombstones[0]
        .suppressed_sources
        .push("message:4812".to_string());
    snapshot.save(&path, ts(2026, 9, 23, 11, 0, 0)).unwrap();
    let loaded = ControlSnapshot::load(&path).unwrap().unwrap();
    assert_eq!(loaded.tombstones.len(), 1, "duplicate tombstones merged");
    assert_eq!(
        loaded.tombstones[0].suppressed_sources,
        ["message:4812", "task:7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90"]
    );
    assert_eq!(loaded.updated_at, ts(2026, 9, 23, 11, 0, 0));
    #[cfg(unix)]
    {
        assert_eq!(fsx::mode_of(&control), Some(0o700));
        assert_eq!(fsx::mode_of(&path), Some(0o600));
    }
    let stray: Vec<_> = std::fs::read_dir(&control)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(stray.is_empty(), "no temp file is left behind");
}

#[test]
fn snapshot_binding_is_checked() {
    let snapshot: ControlSnapshot = serde_json::from_str(SNAPSHOT_EXAMPLE).unwrap();
    assert!(
        snapshot
            .check_binding(
                &snapshot.partition_key,
                &snapshot.owner_key,
                &snapshot.root
            )
            .is_ok()
    );
    assert!(
        snapshot
            .check_binding(&snapshot.partition_key, &snapshot.owner_key, "/elsewhere")
            .is_err()
    );
    assert!(
        snapshot
            .check_binding("home-x", &snapshot.owner_key, &snapshot.root)
            .is_err()
    );
}

#[test]
fn origin_ranks_are_strictly_ordered() {
    let order = [
        Origin::User,
        Origin::ExtractedUser,
        Origin::Imported,
        Origin::External,
        Origin::ExtractedOther,
    ];
    for pair in order.windows(2) {
        assert!(pair[0].rank() > pair[1].rank(), "{:?} > {:?}", pair[0], pair[1]);
    }
    for o in Origin::ALL {
        assert_eq!(Origin::parse(o.as_str()), Some(*o));
    }
}
